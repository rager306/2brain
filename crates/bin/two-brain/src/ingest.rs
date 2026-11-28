//! CLI driver for `2brain ingest` (clean-room MIT).
//!
//! Security & atomicity invariants:
//! - Source files are opened relative to open directory descriptors with `O_NOFOLLOW`
//!   (`rustix::fs::openat`), verified regular via `fstat`, and read with a hard
//!   `Read::take(MAX_SOURCE_BYTES + 1)` streaming cap (zero TOCTOU symlink swaps or OOM).
//! - Refresh targets bind CAS `expected_hash` from the exact gated `VaultSnapshot` (never
//!   re-reading disk right before commit).
//! - Admitted batches commit atomically under one `write.lock` and one Git commit via
//!   `FsWorkspaceStore::commit_ingest_batch` with compensating rollback on failure.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Component, Path, PathBuf};

use rustix::fs::{Mode, OFlags, openat};
use serde_json::json;
use tb_app::ingest::{
    ConvertOptions, IngestAction, IngestDraft, IngestFormat, IngestGateOptions, IngestGateReport,
    convert_source, evaluate_ingest_draft,
};
use tb_fs_git::IngestWriteOp;

const MAX_SOURCE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB hard streaming cap per source file
const MAX_SOURCE_FILES: usize = 500;

pub struct IngestCliArgs {
    pub workspace: PathBuf,
    pub source: PathBuf,
    pub dest: String,
    pub repo: Option<String>,
    pub default_type: Option<String>,
    pub date: Option<String>,
    pub format: String,
    pub allow_unresolved_links: bool,
    pub allow_ambiguous_links: bool,
    pub refresh: bool,
    pub dedup_threshold: Option<f64>,
    pub dry_run: bool,
    pub as_json: bool,
}

struct CollectedSource {
    rel_components: Vec<String>,
    format: IngestFormat,
    label: String,
    stem: String,
}

pub fn run_ingest(args: IngestCliArgs) {
    let explicit_format = match IngestFormat::parse_flag(&args.format) {
        Ok(f) => f,
        Err(err) => {
            eprintln!("error: {err}");
            std::process::exit(2);
        }
    };

    let canonical_ws = match fs::canonicalize(&args.workspace) {
        Ok(c) => c,
        Err(err) => {
            eprintln!(
                "error resolving workspace '{}': {err}",
                args.workspace.display()
            );
            std::process::exit(1);
        }
    };

    let _ = tb_journal::recover_outcome_registry(&canonical_ws);
    let (store, snapshot) = match tb_fs_git::load_snapshot(&canonical_ws) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!(
                "error loading workspace '{}': {err}",
                canonical_ws.display()
            );
            std::process::exit(1);
        }
    };

    let target_repo = match args.repo {
        Some(r) => {
            let Some(meta) = snapshot.members.get(&r) else {
                eprintln!("error: target repo '{r}' is not a member of this workspace");
                std::process::exit(2);
            };
            if meta.role.eq_ignore_ascii_case("dependency") {
                eprintln!(
                    "error: policy/read_only_member: refusing to ingest into read-only dependency repo '{r}'"
                );
                std::process::exit(2);
            }
            r
        }
        None => match tb_app::ingest::resolve_default_writable_repo(&snapshot) {
            Ok(r) => r,
            Err(err) => {
                eprintln!("error: {err}");
                std::process::exit(2);
            }
        },
    };

    let (base_dir_fd, collected) = match open_and_collect_sources(&args.source, explicit_format) {
        Ok(pair) => pair,
        Err(err) => {
            eprintln!("error collecting sources: {err}");
            std::process::exit(1);
        }
    };
    if collected.is_empty() {
        eprintln!(
            "error: no supported source files found under '{}'",
            args.source.display()
        );
        std::process::exit(1);
    }

    let mut drafts: Vec<IngestDraft> = Vec::new();
    let mut conversion_errors: Vec<String> = Vec::new();
    for src in &collected {
        let bytes = match read_regular_file_nofollow(
            base_dir_fd.as_fd(),
            &src.rel_components,
            MAX_SOURCE_BYTES,
        ) {
            Ok(b) => b,
            Err(err) => {
                conversion_errors.push(format!("{}: {err}", src.label));
                continue;
            }
        };
        let conv_opts = ConvertOptions {
            repo: target_repo.clone(),
            dest_dir: args.dest.clone(),
            default_type: args.default_type.clone(),
            captured_date: args.date.clone(),
            source_label: src.label.clone(),
        };
        match convert_source(&src.stem, src.format, &bytes, &conv_opts) {
            Ok(mut produced) => drafts.append(&mut produced),
            Err(err) => conversion_errors.push(err.to_string()),
        }
    }

    // Detect duplicate target paths across the batch before gating.
    let mut batch_locators = BTreeSet::new();
    let mut duplicate_targets = BTreeSet::new();
    for d in &drafts {
        if !batch_locators.insert(d.locator.clone()) {
            duplicate_targets.insert(d.locator.clone());
        }
    }

    let gate_opts = IngestGateOptions {
        allow_unresolved_links: args.allow_unresolved_links,
        allow_ambiguous_links: args.allow_ambiguous_links,
        refresh_existing: args.refresh,
        dedup_score_threshold: args.dedup_threshold,
    };

    let mut reports: Vec<IngestGateReport> = Vec::with_capacity(drafts.len());
    for draft in &drafts {
        let mut rep = evaluate_ingest_draft(&snapshot, draft, &batch_locators, &gate_opts);
        if duplicate_targets.contains(&draft.locator) {
            rep.admitted = false;
            rep.errors.push(format!(
                "batch_collision: multiple sources map to target '{}'",
                draft.locator.path
            ));
        }
        reports.push(rep);
    }

    let all_admitted = conversion_errors.is_empty() && reports.iter().all(|r| r.admitted);
    let mut written = 0usize;
    let mut write_errors: Vec<String> = Vec::new();

    if !args.dry_run && all_admitted && !drafts.is_empty() {
        // Bind refresh expected_hash strictly to the gated VaultSnapshot (never re-reading disk
        // right before write) and commit the entire batch atomically under one write.lock + commit.
        let mut ops = Vec::with_capacity(drafts.len());
        for (draft, rep) in drafts.iter().zip(reports.iter()) {
            let expected_hash = if rep.action == IngestAction::Refresh {
                match snapshot.files.get(&draft.locator) {
                    Some(existing) => Some(existing.short_hash.clone()),
                    None => {
                        write_errors.push(format!(
                            "{}: missing snapshot baseline hash for refresh",
                            draft.locator.path
                        ));
                        continue;
                    }
                }
            } else {
                None
            };
            ops.push(IngestWriteOp {
                locator: draft.locator.clone(),
                content: draft.content.clone(),
                expected_hash,
            });
        }
        if write_errors.is_empty() {
            let commit_msg = format!("2brain: ingest {} note(s)", ops.len());
            match store.commit_ingest_batch(&target_repo, &ops, &commit_msg) {
                Ok(receipts) => written = receipts.len(),
                Err(err) => write_errors.push(err.to_string()),
            }
        }
    }

    let ok = all_admitted && write_errors.is_empty();
    if args.as_json {
        let payload = json!({
            "ok": ok,
            "dry_run": args.dry_run,
            "workspace": canonical_ws.display().to_string(),
            "repo": target_repo,
            "total_drafts": reports.len(),
            "admitted": reports.iter().filter(|r| r.admitted).count(),
            "rejected": reports.iter().filter(|r| !r.admitted).count(),
            "written": written,
            "conversion_errors": conversion_errors,
            "write_errors": write_errors,
            "reports": reports,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).expect("json serialization")
        );
    } else {
        println!(
            "2brain ingest ({}) repo={} drafts={} admitted={} written={}",
            if args.dry_run { "dry-run" } else { "commit" },
            target_repo,
            reports.len(),
            reports.iter().filter(|r| r.admitted).count(),
            written
        );
        for err in &conversion_errors {
            println!("  [convert-error] {err}");
        }
        for rep in &reports {
            let status = if rep.admitted { "OK" } else { "REJECT" };
            println!(
                "  [{status}] {} ({}) <- {}",
                rep.locator.path, rep.action, rep.source
            );
            for err in &rep.errors {
                println!("    error: {err}");
            }
            for warn in &rep.warnings {
                println!("    warn:  {warn}");
            }
            for dup in &rep.near_duplicates {
                println!(
                    "    near-duplicate: {} (score={:.2}, terms={})",
                    dup.locator.path, dup.score, dup.matched_terms
                );
            }
        }
        for err in &write_errors {
            println!("  [write-error] {err}");
        }
    }

    if !ok {
        std::process::exit(1);
    }
}

fn open_dir_nofollow(path: &Path) -> Result<OwnedFd, String> {
    openat(
        rustix::fs::CWD,
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| {
        format!(
            "cannot open directory '{}' without following symlinks: {e}",
            path.display()
        )
    })
}

fn open_and_collect_sources(
    source: &Path,
    explicit_format: Option<IngestFormat>,
) -> Result<(OwnedFd, Vec<CollectedSource>), String> {
    let meta = fs::symlink_metadata(source)
        .map_err(|e| format!("cannot stat '{}': {e}", source.display()))?;
    if meta.file_type().is_symlink() {
        return Err(format!(
            "refusing to follow symlink source '{}'",
            source.display()
        ));
    }
    if meta.file_type().is_file() {
        let fmt = explicit_format
            .or_else(|| {
                source
                    .extension()
                    .and_then(|e| e.to_str())
                    .and_then(IngestFormat::from_extension)
            })
            .ok_or_else(|| {
                format!(
                    "cannot infer ingest format for '{}'; pass --format",
                    source.display()
                )
            })?;
        let file_name = source
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("invalid UTF-8 filename '{}'", source.display()))?
            .to_owned();
        let stem = source
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("note")
            .to_owned();
        let parent = source
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let dir_fd = open_dir_nofollow(parent)?;
        return Ok((
            dir_fd,
            vec![CollectedSource {
                rel_components: vec![file_name.clone()],
                format: fmt,
                label: file_name,
                stem,
            }],
        ));
    }
    if !meta.file_type().is_dir() {
        return Err(format!(
            "'{}' is neither a file nor a directory",
            source.display()
        ));
    }

    let base_fd = open_dir_nofollow(source)?;
    let mut out = Vec::new();
    walk_source_dir(source, source, explicit_format, &mut out)?;
    out.sort_by(|a, b| a.label.cmp(&b.label));
    Ok((base_fd, out))
}

fn walk_source_dir(
    base: &Path,
    dir: &Path,
    explicit_format: Option<IngestFormat>,
    out: &mut Vec<CollectedSource>,
) -> Result<(), String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("cannot read dir '{}': {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry error: {e}"))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| format!("stat failed for '{}': {e}", path.display()))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }
        if meta.file_type().is_dir() {
            walk_source_dir(base, &path, explicit_format, out)?;
        } else if meta.file_type().is_file() {
            let fmt = explicit_format.or_else(|| {
                path.extension()
                    .and_then(|e| e.to_str())
                    .and_then(IngestFormat::from_extension)
            });
            if let Some(format) = fmt {
                if out.len() >= MAX_SOURCE_FILES {
                    return Err(format!(
                        "source directory exceeds maximum batch size ({MAX_SOURCE_FILES} files)"
                    ));
                }
                let rel = path.strip_prefix(base).unwrap_or(&path);
                let mut rel_components = Vec::new();
                for comp in rel.components() {
                    if let Component::Normal(seg) = comp {
                        rel_components.push(seg.to_string_lossy().into_owned());
                    } else {
                        return Err(format!("unsafe relative component in '{}'", rel.display()));
                    }
                }
                let label = rel_components.join("/");
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("note")
                    .to_owned();
                out.push(CollectedSource {
                    rel_components,
                    format,
                    label,
                    stem,
                });
            }
        }
    }
    Ok(())
}

/// Zero-TOCTOU dirfd-relative regular file reader with `O_NOFOLLOW` at every segment
/// and streaming byte-limit enforcement.
fn read_regular_file_nofollow(
    base_dir_fd: rustix::fd::BorrowedFd<'_>,
    components: &[String],
    max_bytes: u64,
) -> Result<Vec<u8>, String> {
    let (file_name, dirs) = components
        .split_last()
        .ok_or_else(|| "empty source path".to_owned())?;
    let mut owned_dir: Option<OwnedFd> = None;
    for dir in dirs {
        let parent = owned_dir.as_ref().map_or(base_dir_fd, AsFd::as_fd);
        let next = openat(
            parent,
            dir.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|e| format!("openat dir '{dir}' failed (symlink blocked): {e}"))?;
        owned_dir = Some(next);
    }
    let parent = owned_dir.as_ref().map_or(base_dir_fd, AsFd::as_fd);
    let fd = openat(
        parent,
        file_name.as_str(),
        OFlags::RDONLY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|e| format!("openat file '{file_name}' failed (symlink blocked): {e}"))?;
    let file = File::from(fd);
    let meta = file.metadata().map_err(|e| format!("fstat failed: {e}"))?;
    if !meta.file_type().is_file() {
        return Err("target is not a regular file".to_owned());
    }
    if meta.len() > max_bytes {
        return Err(format!(
            "exceeds max ingest size ({} > {max_bytes} bytes)",
            meta.len()
        ));
    }
    let mut buf = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    if buf.len() as u64 > max_bytes {
        return Err(format!("exceeds max ingest size (> {max_bytes} bytes)"));
    }
    Ok(buf)
}
