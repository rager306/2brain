//! Driven adapter: SEP-2640 `skill://` catalog (built-in core skills + vault-authored
//! `type: mcp.skill` and promoted `type: pattern` notes) with deterministic SHA-256 digests.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tb_app::VaultSnapshot;
use tb_syntax::FrontmatterValue;

/// One file belonging to a SEP-2640 skill directory.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillFile {
    pub uri: String,
    pub relative_path: String,
    pub mime_type: String,
    pub content: String,
    pub digest: String,
    pub size: u64,
}

impl SkillFile {
    #[must_use]
    pub fn from_markdown(
        uri: impl Into<String>,
        relative_path: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        let content = content.into();
        let bytes = content.as_bytes();
        Self {
            uri: uri.into(),
            relative_path: relative_path.into(),
            mime_type: "text/markdown".to_owned(),
            digest: sha256_prefixed(bytes),
            size: bytes.len() as u64,
            content,
        }
    }
}

/// One SEP-2640 skill descriptor with its verified file manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillEntry {
    pub uri: String,
    pub dir_uri: String,
    pub name: String,
    pub description: String,
    pub source: String,
    pub files: Vec<SkillFile>,
}

/// Complete catalog of built-in + vault-projected skills for a given [`VaultSnapshot`].
#[derive(Clone, Debug)]
pub struct SkillCatalog {
    entries: BTreeMap<String, SkillEntry>,
    files_by_uri: BTreeMap<String, SkillFile>,
    dirs_by_uri: BTreeMap<String, Vec<SkillFile>>,
}

impl SkillCatalog {
    /// Build the skill catalog from built-in core skills plus vault-authored skills.
    #[must_use]
    pub fn from_snapshot(snap: &VaultSnapshot) -> Self {
        let mut entries = BTreeMap::new();
        for builtin in builtin_skills() {
            entries.insert(builtin.uri.clone(), builtin);
        }

        // Dynamic projection: vault notes declaring `type: mcp.skill`, or `type: pattern`
        // with both `trigger` and `procedure` frontmatter fields.
        for file in snap.files.values() {
            let is_mcp_skill = file.declared_type.as_deref() == Some("mcp.skill");
            let is_promoted_pattern =
                file.declared_type.as_deref() == Some("pattern")
                    && file.doc.frontmatter.as_ref().is_some_and(|fm| {
                        fm.contains_key("trigger") && fm.contains_key("procedure")
                    });
            if !is_mcp_skill && !is_promoted_pattern {
                continue;
            }
            let raw_name = file
                .doc
                .frontmatter
                .as_ref()
                .and_then(|fm| fm.get("name"))
                .and_then(fm_as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| slug_from_path(&file.locator.path));
            let slug = sanitize_skill_name(&raw_name);
            if slug.is_empty() {
                continue;
            }
            let description = file.description.clone().unwrap_or_else(|| {
                format!(
                    "Vault skill projected from {}/{}",
                    file.locator.repo, file.locator.path
                )
            });
            let dir_uri = format!("skill://vault/{slug}/");
            let uri = format!("{dir_uri}SKILL.md");
            if entries.contains_key(&uri) {
                continue;
            }
            let rendered = format!(
                "---\nname: {slug}\ndescription: {}\n---\n\n{}",
                yaml_quote(&description),
                file.body().trim_start()
            );
            let sf = SkillFile::from_markdown(&uri, "SKILL.md", rendered);
            entries.insert(
                uri.clone(),
                SkillEntry {
                    uri,
                    dir_uri,
                    name: slug,
                    description,
                    source: format!("{}/{}", file.locator.repo, file.locator.path),
                    files: vec![sf],
                },
            );
        }

        let mut files_by_uri = BTreeMap::new();
        let mut dirs_by_uri = BTreeMap::new();
        for entry in entries.values() {
            dirs_by_uri.insert(entry.dir_uri.clone(), entry.files.clone());
            for f in &entry.files {
                files_by_uri.insert(f.uri.clone(), f.clone());
            }
        }

        Self {
            entries,
            files_by_uri,
            dirs_by_uri,
        }
    }

    pub fn list(&self) -> impl Iterator<Item = &SkillEntry> {
        self.entries.values()
    }

    #[must_use]
    pub fn get(&self, uri: &str) -> Option<&SkillEntry> {
        self.entries.get(uri)
    }

    #[must_use]
    pub fn read_file(&self, uri: &str) -> Option<&SkillFile> {
        self.files_by_uri.get(uri)
    }

    #[must_use]
    pub fn read_directory(&self, dir_uri: &str) -> Option<&[SkillFile]> {
        let normalized = if dir_uri.ends_with('/') {
            dir_uri.to_owned()
        } else {
            format!("{dir_uri}/")
        };
        self.dirs_by_uri.get(&normalized).map(Vec::as_slice)
    }
}

fn builtin_skills() -> Vec<SkillEntry> {
    let defs = [
        (
            "grounded-answer",
            "Answer questions from revision-bound evidence bundles without fabrication.",
            GROUNDED_ANSWER_SKILL,
        ),
        (
            "safe-edit",
            "Modify vault notes through full-file tb_content reads, expected_hash CAS, and post-write diagnostics.",
            SAFE_EDIT_SKILL,
        ),
        (
            "cognitive-explore",
            "Navigate graph structure, hubs, neighborhoods, and structural gaps systematically.",
            COGNITIVE_EXPLORE_SKILL,
        ),
        (
            "contradiction-audit",
            "Surface tensions, broken links, and schema diagnostics without averaging conflicting claims.",
            CONTRADICTION_AUDIT_SKILL,
        ),
        (
            "session-handoff",
            "Produce a revision-cited handoff with decisions, evidence paths, unverified items, and next tool action.",
            SESSION_HANDOFF_SKILL,
        ),
    ];

    defs.into_iter()
        .map(|(name, desc, body)| {
            let dir_uri = format!("skill://2brain/{name}/");
            let uri = format!("{dir_uri}SKILL.md");
            let full = format!("---\nname: {name}\ndescription: {desc}\n---\n\n{body}");
            let sf = SkillFile::from_markdown(&uri, "SKILL.md", full);
            SkillEntry {
                uri,
                dir_uri,
                name: name.to_owned(),
                description: desc.to_owned(),
                source: "builtin".to_owned(),
                files: vec![sf],
            }
        })
        .collect()
}

fn sha256_prefixed(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut out = String::with_capacity(7 + 64);
    out.push_str("sha256:");
    for b in hash {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn fm_as_str(v: &FrontmatterValue) -> Option<&str> {
    match v {
        FrontmatterValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

fn slug_from_path(path: &str) -> String {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".md").unwrap_or(file).to_owned()
}

fn sanitize_skill_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

fn yaml_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

const GROUNDED_ANSWER_SKILL: &str = "\
# Grounded Answer

1. Start with `tb_overview` and `tb_types` when you do not yet hold the workspace vocabulary.
2. Search with `tb_search` (BM25 + proximity) or `tb_grep` (exact literal substring; never confuse the two).
3. Read full notes with `tb_content` before citing; never build claims solely from search excerpts.
4. Preserve explicit uncertainty: report missing coverage directly rather than fabricating links.
5. Cite every claim by `(repo, path)` and exact SHA-256 revision when precision matters.
";

const SAFE_EDIT_SKILL: &str = "\
# Safe Edit

1. Read the entire target file via `tb_content` without `max_chars` truncation (partial reads never arm writes).
2. Pass the returned `hash` / `expected_hash` to `tb_write_file` or `tb_edit_file`.
3. Never retry an `outcome_unknown` mutation blindly; inspect `tb_mutation_outcome` or `tb_reconcile` first.
4. Confirm clean schema and link state with `tb_diagnostics` scoped to the changed path.
";

const COGNITIVE_EXPLORE_SKILL: &str = "\
# Cognitive Explore

1. Start with `tb_orient` (when `--allow-cognitive` is enabled) or `tb_overview`, `tb_hubs`, and `tb_graph_health`.
2. Run `tb_analyze_topology` for Brandes gateways, Leiden+NMI+Null Callon communities, and Peel-the-Onion hidden hubs.
3. Run `tb_detect_gaps` for Dual-Layer (Link ⊕ Rare-IDF) structural blind spots and `tb_fragility_report` for overloaded MOC hubs.
4. Compare snapshots across git history with `tb_diff_graphs` (supports `async_task: true` under MCP 2.0 SEP-2663 tasks).
5. Expand anchor concepts with `tb_neighborhood` and assemble bounded evidence packs with `tb_context`; audit draft answers via `tb_audit_bias`.
";

const CONTRADICTION_AUDIT_SKILL: &str = "\
# Contradiction Audit

1. Run `tb_diagnostics` for schema violations, unresolved links, and ambiguous stems.
2. Run `tb_audit_contradictions` (with optional `as_of` ISO date) to surface explicit `conflicts_with` edges, implicit polarity clashes, supersession cycles, and expired active claims.
3. Preserve Polysingularity: cross-check both poles via `tb_content` and present tensions explicitly rather than averaging them.
4. Mark unverified dimensions honestly (`unassessed` / `unavailable`) instead of reporting empty conflict lists.
";

const SESSION_HANDOFF_SKILL: &str = "\
# Session Handoff

1. Record decisions in one concrete sentence each.
2. Cite evidence by `(repo, path)` so the next agent can re-open via `tb_content`.
3. List unverified checks, open diagnostics, and unresolved `outcome_unknown` mutations.
4. Name one concrete next action with its tool, member, and path.
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_catalog_exposes_five_verified_skills() {
        let catalog = SkillCatalog::from_snapshot(&VaultSnapshot::empty());
        let skills: Vec<&SkillEntry> = catalog.list().collect();
        assert_eq!(skills.len(), 5);
        for s in skills {
            assert!(s.uri.starts_with("skill://2brain/"));
            assert!(s.uri.ends_with("/SKILL.md"));
            assert_eq!(s.files.len(), 1);
            let f = &s.files[0];
            assert!(f.digest.starts_with("sha256:"));
            assert_eq!(f.digest.len(), 7 + 64);
            assert_eq!(f.digest, sha256_prefixed(f.content.as_bytes()));
            assert_eq!(catalog.read_file(&f.uri), Some(f));
            assert_eq!(catalog.read_directory(&s.dir_uri).map(<[_]>::len), Some(1));
        }
    }
}
