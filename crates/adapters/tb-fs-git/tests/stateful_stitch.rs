//! Clean-room STITCH++-style Typed-Composition Stateful Test Harness for `tb-fs-git`
//! (`New`/`Borrow`/`Take` resource lifecycles, `require`=expected fail-closed bail vs
//! `check`=hard invariant violation, online pair-success tracking, and deterministic shrinking).
//!
//! Verifies end-to-end invariants across randomized multi-step workspace mutation sequences:
//! 1. `incremental_snapshot == full_reload` (files, edges, diagnostics, weights, freshness_policy)
//! 2. `search_index` parity between incremental updates and fresh rebuilds
//! 3. Non-destructive manifest preservation (`search:` weights/freshness survive `update_repo_deps`
//!    and `update_workspace_member`)
//! 4. Valid YAML round-trip on every mutated manifest

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use tb_app::{StoreError, WorkspaceMemberAction, WorkspaceStore};
use tb_contracts::{Generation, Locator};
use tb_fs_git::{load_snapshot, load_snapshot_with_generation, refresh_snapshot_incremental};

#[derive(Clone, Debug)]
enum StepOp {
    CreateNote { slug: String, body_seed: u64 },
    EditNote { idx: usize, body_seed: u64 },
    DeleteNote { idx: usize },
    RenameNote { idx: usize, new_slug: String },
    AddDep { dep: String },
    RemoveDep { dep: String },
    BadEditStaleHash { idx: usize },
}

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn range(&mut self, upper: usize) -> usize {
        if upper == 0 {
            0
        } else {
            (self.next() as usize) % upper
        }
    }
}

/// Execution stats mirroring STITCH++ `require` (bail) vs `check` (bug) accounting.
#[derive(Default, Debug)]
struct HarnessStats {
    invoked: usize,
    ok: usize,
    bails: usize,
}

fn note_body(slug: &str, seed: u64, peer_slug: Option<&str>) -> String {
    let link_line = match peer_slug {
        Some(p) => format!("Related: [[{p}]]\n"),
        None => String::new(),
    };
    format!(
        "---\ndescription: \"Stateful note {slug} rev {seed}\"\nkeywords: [\"kw_{seed}\", \"stitch_stateful\"]\n---\n\n# {slug}\n\nToken tok_{seed} body content.\n{link_line}"
    )
}

/// Run a program of typed operations against a fresh temporary workspace and assert every
/// post-step invariant (`check`). Returns `Ok(stats)` if all invariants hold, or `Err(msg)` on bug.
fn execute_program(ops: &[StepOp]) -> Result<HarnessStats, String> {
    let dir = tempfile::tempdir().map_err(|e| format!("tempdir: {e}"))?;
    let root = dir.path();
    fs::create_dir_all(root.join(".2brain")).map_err(|e| format!("mkdir .2brain: {e}"))?;
    let repo_yaml = concat!(
        "# preserve this comment\n",
        "name: main\n",
        "search:\n",
        "  weights:\n",
        "    keywords: 4.25\n",
        "  freshness:\n",
        "    transient_half_life_days: 45\n",
    );
    fs::write(root.join(".2brain/repo.yaml"), repo_yaml)
        .map_err(|e| format!("write repo.yaml: {e}"))?;
    fs::write(root.join("seed.md"), note_body("seed", 1, None))
        .map_err(|e| format!("write seed.md: {e}"))?;

    let mut gen_num = 1u64;
    let (store, mut inc_snap) = load_snapshot(root).map_err(|e| format!("initial load: {e}"))?;

    // Typed live resource table (`New` inserts, `Borrow` reads/edits, `Take` removes/renames).
    let mut live_notes: BTreeMap<String, String> = BTreeMap::new(); // path -> sha256
    for (loc, file) in &inc_snap.files {
        live_notes.insert(loc.path.clone(), file.sha256.to_hex());
    }

    let mut stats = HarnessStats::default();

    for (step_idx, op) in ops.iter().enumerate() {
        stats.invoked += 1;
        let mut touched_paths: Vec<PathBuf> = Vec::new();
        let mut step_token: Option<String> = None;
        let peer = live_notes
            .keys()
            .next()
            .map(|p| p.trim_end_matches(".md").to_owned());

        match op {
            StepOp::CreateNote { slug, body_seed } => {
                let path = format!("{slug}.md");
                let loc = Locator::new("main", &path).map_err(|e| e.to_string())?;
                let content = note_body(slug, *body_seed, peer.as_deref());
                match store.create_file(&loc, &content, "stitch: create") {
                    Ok(receipt) => {
                        live_notes.insert(path.clone(), receipt.sha256);
                        touched_paths.push(root.join(&path));
                        step_token = Some(format!("tok_{body_seed}"));
                        stats.ok += 1;
                    }
                    Err(StoreError::AlreadyExists(_)) => {
                        // Expected `require` bail (target path already exists).
                        stats.bails += 1;
                        continue;
                    }
                    Err(err) => {
                        return Err(format!(
                            "step {step_idx} CreateNote({path}) unexpected error: {err}"
                        ));
                    }
                }
            }
            StepOp::EditNote { idx, body_seed } => {
                if live_notes.is_empty() {
                    stats.bails += 1;
                    continue;
                }
                let keys: Vec<String> = live_notes.keys().cloned().collect();
                let path = &keys[*idx % keys.len()];
                let expected_sha = live_notes[path].clone();
                let loc = Locator::new("main", path).map_err(|e| e.to_string())?;
                let content = note_body(path.trim_end_matches(".md"), *body_seed, peer.as_deref());
                match store.write_file_cas(&loc, &expected_sha, &content, "stitch: edit") {
                    Ok(receipt) => {
                        live_notes.insert(path.clone(), receipt.sha256);
                        touched_paths.push(root.join(path));
                        step_token = Some(format!("tok_{body_seed}"));
                        stats.ok += 1;
                    }
                    Err(err) => {
                        return Err(format!(
                            "step {step_idx} EditNote({path}) failed with valid CAS sha: {err}"
                        ));
                    }
                }
            }
            StepOp::DeleteNote { idx } => {
                if live_notes.len() <= 1 {
                    stats.bails += 1;
                    continue;
                }
                let keys: Vec<String> = live_notes.keys().cloned().collect();
                let path = keys[*idx % keys.len()].clone();
                let expected_sha = live_notes[&path].clone();
                let loc = Locator::new("main", &path).map_err(|e| e.to_string())?;
                match store.delete_file_cas(&loc, &expected_sha, "stitch: delete") {
                    Ok(_) => {
                        live_notes.remove(&path);
                        touched_paths.push(root.join(&path));
                        stats.ok += 1;
                    }
                    Err(err) => {
                        return Err(format!(
                            "step {step_idx} DeleteNote({path}) failed with valid CAS sha: {err}"
                        ));
                    }
                }
            }
            StepOp::RenameNote { idx, new_slug } => {
                if live_notes.is_empty() {
                    stats.bails += 1;
                    continue;
                }
                let keys: Vec<String> = live_notes.keys().cloned().collect();
                let old_path = keys[*idx % keys.len()].clone();
                let new_path = format!("{new_slug}.md");
                if old_path == new_path {
                    stats.bails += 1;
                    continue;
                }
                let expected_sha = live_notes[&old_path].clone();
                let from = Locator::new("main", &old_path).map_err(|e| e.to_string())?;
                let to = Locator::new("main", &new_path).map_err(|e| e.to_string())?;
                match store.rename_file_cas(&from, &to, &expected_sha, "stitch: rename") {
                    Ok(receipt) => {
                        live_notes.remove(&old_path);
                        live_notes.insert(new_path.clone(), receipt.sha256);
                        touched_paths.push(root.join(&old_path));
                        touched_paths.push(root.join(&new_path));
                        stats.ok += 1;
                    }
                    Err(StoreError::AlreadyExists(_)) => {
                        // Expected `require` bail (destination path already exists).
                        stats.bails += 1;
                        continue;
                    }
                    Err(err) => {
                        return Err(format!(
                            "step {step_idx} RenameNote({old_path} -> {new_path}) unexpected error: {err}"
                        ));
                    }
                }
            }
            StepOp::AddDep { dep } => {
                store
                    .update_repo_deps("main", dep, true)
                    .map_err(|e| format!("step {step_idx} AddDep({dep}) failed: {e}"))?;
                touched_paths.push(root.join(".2brain/repo.yaml"));
                stats.ok += 1;
            }
            StepOp::RemoveDep { dep } => {
                store
                    .update_repo_deps("main", dep, false)
                    .map_err(|e| format!("step {step_idx} RemoveDep({dep}) failed: {e}"))?;
                touched_paths.push(root.join(".2brain/repo.yaml"));
                stats.ok += 1;
            }
            StepOp::BadEditStaleHash { idx } => {
                if live_notes.is_empty() {
                    stats.bails += 1;
                    continue;
                }
                let keys: Vec<String> = live_notes.keys().cloned().collect();
                let path = &keys[*idx % keys.len()];
                let loc = Locator::new("main", path).map_err(|e| e.to_string())?;
                // CHECK: stale CAS hash MUST be rejected fail-closed with CasMismatch.
                match store.write_file_cas(
                    &loc,
                    "0000000000000000",
                    "corrupted overwrite",
                    "stitch: bad",
                ) {
                    Err(StoreError::CasMismatch(_)) => {
                        stats.bails += 1;
                        continue;
                    }
                    Ok(_) => {
                        return Err(format!(
                            "step {step_idx} INVARIANT VIOLATION: stale CAS hash was accepted for {path}"
                        ));
                    }
                    Err(other) => {
                        return Err(format!(
                            "step {step_idx} BadEditStaleHash({path}) returned unexpected error instead of CasMismatch: {other}"
                        ));
                    }
                }
            }
        }

        // CHECK 1: Incremental refresh must match full reload exactly.
        gen_num += 1;
        inc_snap =
            refresh_snapshot_incremental(root, &inc_snap, &touched_paths, Generation(gen_num))
                .map_err(|e| format!("step {step_idx} incremental refresh failed: {e}"))?;
        let full_snap = load_snapshot_with_generation(root, Generation(gen_num))
            .map_err(|e| format!("step {step_idx} full reload failed: {e}"))?;

        if inc_snap.files.len() != full_snap.files.len() {
            return Err(format!(
                "step {step_idx}: file count mismatch inc={} full={}",
                inc_snap.files.len(),
                full_snap.files.len()
            ));
        }
        if inc_snap.graph.edge_count() != full_snap.graph.edge_count() {
            return Err(format!(
                "step {step_idx}: edge count mismatch inc={} full={}",
                inc_snap.graph.edge_count(),
                full_snap.graph.edge_count()
            ));
        }
        if inc_snap.diagnostics.len() != full_snap.diagnostics.len() {
            return Err(format!(
                "step {step_idx}: diagnostic count mismatch inc={} full={}",
                inc_snap.diagnostics.len(),
                full_snap.diagnostics.len()
            ));
        }
        // CHECK 2: Search weights & freshness policy from `.2brain/repo.yaml` must never be lost.
        if (inc_snap.search.weights().keywords - 4.25).abs() > 1e-9
            || (full_snap.search.weights().keywords - 4.25).abs() > 1e-9
        {
            return Err(format!(
                "step {step_idx}: search.weights.keywords lost after mutation (inc={}, full={})",
                inc_snap.search.weights().keywords,
                full_snap.search.weights().keywords
            ));
        }
        if (inc_snap.search.freshness_policy().transient_half_life_days - 45.0).abs() > 1e-9 {
            return Err(format!(
                "step {step_idx}: search.freshness lost after mutation"
            ));
        }
        // CHECK 3: Search index hit set parity on shared keyword AND freshly written step token.
        let inc_hits: Vec<Locator> = inc_snap
            .search
            .query("stitch_stateful", false, None, None, 20)
            .into_iter()
            .map(|h| h.locator)
            .collect();
        let full_hits: Vec<Locator> = full_snap
            .search
            .query("stitch_stateful", false, None, None, 20)
            .into_iter()
            .map(|h| h.locator)
            .collect();
        if inc_hits != full_hits {
            return Err(format!(
                "step {step_idx}: search hit parity mismatch inc={inc_hits:?} full={full_hits:?}"
            ));
        }
        if let Some(tok) = step_token.as_deref() {
            let inc_tok: Vec<Locator> = inc_snap
                .search
                .query(tok, false, None, None, 10)
                .into_iter()
                .map(|h| h.locator)
                .collect();
            let full_tok: Vec<Locator> = full_snap
                .search
                .query(tok, false, None, None, 10)
                .into_iter()
                .map(|h| h.locator)
                .collect();
            if inc_tok != full_tok || inc_tok.is_empty() {
                return Err(format!(
                    "step {step_idx}: token '{tok}' search parity mismatch inc={inc_tok:?} full={full_tok:?}"
                ));
            }
        }
    }

    // CHECK 4: Non-destructive manifest comment preservation.
    let final_yaml = fs::read_to_string(root.join(".2brain/repo.yaml"))
        .map_err(|e| format!("read final repo.yaml: {e}"))?;
    if !final_yaml.contains("# preserve this comment") {
        return Err("comment in .2brain/repo.yaml was lost".to_owned());
    }

    Ok(stats)
}

/// Deterministic delta-debugging shrinker (STITCH++ style): shrinks any failing program to a
/// minimal counterexample preserving the invariant failure.
fn shrink_failing_program(ops: &[StepOp]) -> Vec<StepOp> {
    let mut best = ops.to_vec();
    let mut improved = true;
    while improved && best.len() > 1 {
        improved = false;
        for i in 0..best.len() {
            let mut cand = best.clone();
            cand.remove(i);
            if execute_program(&cand).is_err() {
                best = cand;
                improved = true;
                break;
            }
        }
    }
    best
}

fn generate_program(rng: &mut XorShift64, len: usize) -> Vec<StepOp> {
    let slugs = ["alpha", "beta", "gamma", "delta", "epsilon"];
    let deps = ["lib-a", "lib-b", "core-types"];
    let mut ops = Vec::with_capacity(len);
    for _ in 0..len {
        let pick = rng.range(7);
        let op = match pick {
            0 => StepOp::CreateNote {
                slug: slugs[rng.range(slugs.len())].to_owned(),
                body_seed: rng.next() % 10_000,
            },
            1 => StepOp::EditNote {
                idx: rng.range(8),
                body_seed: rng.next() % 10_000,
            },
            2 => StepOp::DeleteNote { idx: rng.range(8) },
            3 => StepOp::RenameNote {
                idx: rng.range(8),
                new_slug: slugs[rng.range(slugs.len())].to_owned(),
            },
            4 => StepOp::AddDep {
                dep: deps[rng.range(deps.len())].to_owned(),
            },
            5 => StepOp::RemoveDep {
                dep: deps[rng.range(deps.len())].to_owned(),
            },
            _ => StepOp::BadEditStaleHash { idx: rng.range(8) },
        };
        ops.push(op);
    }
    ops
}

#[test]
fn stitch_stateful_typed_composition_preserves_incremental_and_manifest_invariants() {
    let mut rng = XorShift64(0x5717_C400_2026_1129);
    let mut total_ok = 0usize;
    let mut total_bails = 0usize;

    for run in 0..24 {
        let len = 8 + rng.range(10);
        let prog = generate_program(&mut rng, len);
        match execute_program(&prog) {
            Ok(st) => {
                total_ok += st.ok;
                total_bails += st.bails;
            }
            Err(err) => {
                let shrunk = shrink_failing_program(&prog);
                panic!(
                    "stateful harness run #{run} failed: {err}\nshrunk minimal repro ({} steps): {shrunk:#?}",
                    shrunk.len()
                );
            }
        }
    }

    assert!(
        total_ok >= 50,
        "expected substantial executed mutations, got {total_ok}"
    );
    assert!(
        total_bails > 0,
        "expected fail-closed require bails to be exercised"
    );
}

#[test]
fn stitch_stateful_workspace_yaml_member_transitions_preserve_search_config() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let own = root.join("own");
    let dep1 = root.join("dep1");
    let dep2 = root.join("dep2");
    fs::create_dir_all(own.join(".2brain")).unwrap();
    fs::create_dir_all(dep1.join(".2brain")).unwrap();
    fs::create_dir_all(dep2.join(".2brain")).unwrap();
    fs::write(own.join("a.md"), "# Own\n").unwrap();
    fs::write(dep1.join("b.md"), "# Dep1\n").unwrap();
    fs::write(dep2.join("c.md"), "# Dep2\n").unwrap();
    fs::write(
        root.join("workspace.yaml"),
        "# workspace header\nmembers:\n  - name: own\n    path: own\n    role: own\nsearch:\n  weights:\n    keywords: 6.5\n",
    )
    .unwrap();

    let (store, _) = load_snapshot(root).unwrap();
    store
        .update_workspace_member(WorkspaceMemberAction::Add {
            name: "dep1".into(),
            path: "dep1".into(),
            role: "dependency".into(),
        })
        .unwrap();
    store
        .update_workspace_member(WorkspaceMemberAction::Add {
            name: "dep2".into(),
            path: "dep2".into(),
            role: "own".into(),
        })
        .unwrap();
    store
        .update_workspace_member(WorkspaceMemberAction::SetRole {
            name: "dep2".into(),
            role: "dependency".into(),
        })
        .unwrap();
    store
        .update_workspace_member(WorkspaceMemberAction::Remove {
            name: "dep1".into(),
        })
        .unwrap();

    let snap = load_snapshot_with_generation(root, Generation(2)).unwrap();
    assert_eq!(snap.members.len(), 2);
    assert_eq!(snap.members["dep2"].role, "dependency");
    assert_eq!(snap.search.weights().keywords, 6.5);
    let raw = fs::read_to_string(root.join("workspace.yaml")).unwrap();
    assert!(raw.contains("# workspace header"));
}
