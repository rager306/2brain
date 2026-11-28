//! Clean-room, pure-application ingest pipeline (0 I/O).
//!
//! Converts raw source payloads (`.md`, `.txt`, `.html`, `.csv`, `.json`, `.jsonl`)
//! into typed Markdown notes with safe YAML 1.2 frontmatter, then runs a fail-closed
//! pre-commit gate:
//! 1. Strict UTF-8 & Markdown/YAML parsing (`tb-syntax`, SEM-04)
//! 2. Workspace `*.type.yaml` schema validation (`tb-schema`)
//! 3. Outgoing `[[wikilink]]` resolution against `VaultSnapshot` + current batch
//! 4. Near-duplicate detection via BM25 (`tb-search`)
//! 5. Stale / placeholder description lint

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tb_contracts::Locator;
use tb_schema::{Diagnostic, Severity};
use tb_search::distinct_terms;
use tb_syntax::{FrontmatterValue, parse_document, tokenize};
use thiserror::Error;

use crate::{LinkResolution, VaultSnapshot};

/// One atomic file write inside `WorkspaceStore::commit_ingest_batch`.
#[derive(Clone, Debug)]
pub struct IngestWriteOp {
    pub locator: Locator,
    pub content: String,
    /// `None` for create (fails if target exists); `Some(short_hash)` for CAS refresh.
    pub expected_hash: Option<String>,
}

/// Classification for workspace-wide frontmatter description quality audits.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptionIssueKind {
    Missing,
    Weak,
    Stale,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DescriptionAuditSummary {
    pub total: usize,
    pub ok: usize,
    pub missing: usize,
    pub weak: usize,
    pub stale: usize,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DescriptionAuditFinding {
    pub locator: Locator,
    pub kind: DescriptionIssueKind,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DescriptionAuditReport {
    pub summary: DescriptionAuditSummary,
    pub findings: Vec<DescriptionAuditFinding>,
}

/// Audit frontmatter `description` freshness and quality across a [`VaultSnapshot`] (0 I/O).
#[must_use]
pub fn audit_descriptions(
    snapshot: &VaultSnapshot,
    repo: Option<&str>,
    limit: usize,
) -> DescriptionAuditReport {
    let mut summary = DescriptionAuditSummary::default();
    let mut findings = Vec::new();

    for (loc, file) in &snapshot.files {
        if let Some(r) = repo
            && loc.repo != r
        {
            continue;
        }
        summary.total += 1;
        let desc_opt = file.description.as_deref().map(str::trim);
        let issue = match desc_opt {
            None | Some("") => {
                summary.missing += 1;
                Some((DescriptionIssueKind::Missing, None))
            }
            Some(desc) => {
                if is_weak_description(desc) {
                    summary.weak += 1;
                    Some((DescriptionIssueKind::Weak, Some(truncate_chars(desc, 96))))
                } else if is_lexically_disconnected(desc, file.body()) {
                    summary.stale += 1;
                    Some((DescriptionIssueKind::Stale, Some(truncate_chars(desc, 96))))
                } else {
                    summary.ok += 1;
                    None
                }
            }
        };
        if let Some((kind, description)) = issue
            && findings.len() < limit
        {
            findings.push(DescriptionAuditFinding {
                locator: loc.clone(),
                kind,
                description,
            });
        }
    }

    DescriptionAuditReport { summary, findings }
}

/// Select the unique writable member repository (`own`, `primary`, or legacy `editable`) when
/// callers omit `repo`. Fails closed if zero or multiple writable members exist so writes can
/// never silently land in a `dependency` member.
pub fn resolve_default_writable_repo(snapshot: &VaultSnapshot) -> Result<String, String> {
    let mut writable: Vec<&str> = snapshot
        .members
        .values()
        .filter(|m| matches!(m.role.as_str(), "own" | "primary" | "editable"))
        .map(|m| m.repo.as_str())
        .collect();
    writable.sort_unstable();
    match writable.len() {
        1 => Ok(writable[0].to_owned()),
        0 => Err(
            "no writable ('own'/'primary') workspace member found; pass 'repo' explicitly"
                .to_owned(),
        ),
        _ => Err(format!(
            "ambiguous default repo across multiple writable members [{}]; pass 'repo' explicitly",
            writable.join(", ")
        )),
    }
}

/// Supported clean-room ingest formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestFormat {
    Markdown,
    Text,
    Html,
    Csv,
    Json,
    Jsonl,
}

impl IngestFormat {
    /// Infer format from a file path extension.
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "txt" | "text" | "log" => Some(Self::Text),
            "html" | "htm" => Some(Self::Html),
            "csv" => Some(Self::Csv),
            "json" => Some(Self::Json),
            "jsonl" | "ndjson" => Some(Self::Jsonl),
            _ => None,
        }
    }

    /// Parse explicit `--format` flag value.
    pub fn parse_flag(raw: &str) -> Result<Option<Self>, IngestError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(None),
            "md" | "markdown" => Ok(Some(Self::Markdown)),
            "txt" | "text" => Ok(Some(Self::Text)),
            "html" | "htm" => Ok(Some(Self::Html)),
            "csv" => Ok(Some(Self::Csv)),
            "json" => Ok(Some(Self::Json)),
            "jsonl" | "ndjson" => Ok(Some(Self::Jsonl)),
            other => Err(IngestError::UnsupportedFormat(other.to_owned())),
        }
    }
}

#[derive(Debug, Error)]
pub enum IngestError {
    #[error("unsupported ingest format '{0}' (expected auto|md|txt|html|csv|json|jsonl)")]
    UnsupportedFormat(String),
    #[error("source '{source_label}' contains invalid UTF-8 bytes (SEM-04)")]
    InvalidUtf8 { source_label: String },
    #[error("source '{source_label}' has malformed frontmatter: {reason}")]
    InvalidFrontmatter {
        source_label: String,
        reason: String,
    },
    #[error("source '{source_label}' has malformed CSV: {reason}")]
    InvalidCsv {
        source_label: String,
        reason: String,
    },
    #[error("source '{source_label}' has invalid JSON: {reason}")]
    InvalidJson {
        source_label: String,
        reason: String,
    },
    #[error("source '{source_label}' is empty or contains no records")]
    EmptySource { source_label: String },
    #[error("invalid destination directory '{0}'")]
    InvalidDestination(String),
    #[error("invalid target locator '{path}': {reason}")]
    InvalidLocator { path: String, reason: String },
}

/// Options controlling conversion and frontmatter synthesis.
#[derive(Clone, Debug, Default)]
pub struct ConvertOptions {
    /// Target repository name.
    pub repo: String,
    /// Destination directory inside the repo (e.g. `"inbox"` or `""` for repo root).
    pub dest_dir: String,
    /// Default frontmatter `type` when the source does not already specify one.
    pub default_type: Option<String>,
    /// Optional capture date (`YYYY-MM-DD`).
    pub captured_date: Option<String>,
    /// Optional source provenance label (relative filename / URL).
    pub source_label: String,
}

/// A converted, ready-to-validate note draft.
#[derive(Clone, Debug)]
pub struct IngestDraft {
    pub locator: Locator,
    pub source_label: String,
    pub format: IngestFormat,
    pub content: String,
}

/// Options controlling the fail-closed pre-commit ingest gate.
#[derive(Clone, Debug, Default)]
pub struct IngestGateOptions {
    /// When `false` (default), unresolved outgoing `[[wikilinks]]` block admission.
    pub allow_unresolved_links: bool,
    /// When `false` (default), ambiguous stem `[[wikilinks]]` block admission.
    pub allow_ambiguous_links: bool,
    /// When `true`, existing targets may be refreshed via CAS `write_file`.
    pub refresh_existing: bool,
    /// Optional BM25 score threshold above which near-duplicates block admission.
    pub dedup_score_threshold: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DuplicateCandidate {
    pub locator: Locator,
    pub score: f64,
    pub matched_terms: usize,
}

/// Gate verdict for a single [`IngestDraft`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IngestGateReport {
    pub locator: Locator,
    pub source: String,
    pub format: IngestFormat,
    pub admitted: bool,
    pub action: String,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub schema_diagnostics: Vec<Diagnostic>,
    pub unresolved_links: Vec<String>,
    pub ambiguous_links: Vec<String>,
    pub near_duplicates: Vec<DuplicateCandidate>,
    pub stale_description: bool,
}

/// Convert raw bytes of a source payload into one or more typed Markdown [`IngestDraft`]s.
pub fn convert_source(
    stem_hint: &str,
    format: IngestFormat,
    bytes: &[u8],
    opts: &ConvertOptions,
) -> Result<Vec<IngestDraft>, IngestError> {
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(IngestError::InvalidUtf8 {
            source_label: opts.source_label.clone(),
        });
    }
    let text = std::str::from_utf8(bytes).map_err(|_| IngestError::InvalidUtf8 {
        source_label: opts.source_label.clone(),
    })?;
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.trim().is_empty() {
        return Err(IngestError::EmptySource {
            source_label: opts.source_label.clone(),
        });
    }

    let dest = normalize_dest_dir(&opts.dest_dir)?;

    match format {
        IngestFormat::Markdown => {
            let content = normalize_markdown(&normalized, stem_hint, opts)?;
            let locator = build_target_locator(&opts.repo, &dest, stem_hint)?;
            Ok(vec![IngestDraft {
                locator,
                source_label: opts.source_label.clone(),
                format,
                content,
            }])
        }
        IngestFormat::Text => {
            let title = extract_plain_title(&normalized, stem_hint);
            let desc = synthesize_description(&normalized, &title);
            let mut fm = BTreeMap::new();
            insert_common_frontmatter(&mut fm, opts, &desc);
            let body = if normalized.trim_start().starts_with("# ") {
                normalized.trim().to_owned()
            } else {
                format!("# {title}\n\n{}", normalized.trim())
            };
            let content = render_note(&fm, &body);
            let locator = build_target_locator(&opts.repo, &dest, stem_hint)?;
            Ok(vec![IngestDraft {
                locator,
                source_label: opts.source_label.clone(),
                format,
                content,
            }])
        }
        IngestFormat::Html => {
            let (html_title, md_body) = html_to_markdown(&normalized);
            let title = html_title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| humanize_stem(stem_hint));
            let desc = synthesize_description(&md_body, &title);
            let mut fm = BTreeMap::new();
            insert_common_frontmatter(&mut fm, opts, &desc);
            let body = if md_body.trim_start().starts_with("# ") {
                md_body
            } else {
                format!("# {title}\n\n{md_body}")
            };
            let content = render_note(&fm, &body);
            let locator = build_target_locator(&opts.repo, &dest, stem_hint)?;
            Ok(vec![IngestDraft {
                locator,
                source_label: opts.source_label.clone(),
                format,
                content,
            }])
        }
        IngestFormat::Csv => {
            let rows = parse_csv(&normalized, &opts.source_label)?;
            if rows.is_empty() {
                return Err(IngestError::EmptySource {
                    source_label: opts.source_label.clone(),
                });
            }
            let title = humanize_stem(stem_hint);
            let table = render_csv_markdown_table(&rows);
            let col_count = rows[0].len();
            let data_rows = rows.len().saturating_sub(1);
            let desc =
                format!("Tabular import of {title} ({data_rows} rows across {col_count} columns).");
            let mut fm = BTreeMap::new();
            insert_common_frontmatter(&mut fm, opts, &desc);
            let body = format!("# {title}\n\n{table}");
            let content = render_note(&fm, &body);
            let locator = build_target_locator(&opts.repo, &dest, stem_hint)?;
            Ok(vec![IngestDraft {
                locator,
                source_label: opts.source_label.clone(),
                format,
                content,
            }])
        }
        IngestFormat::Json => {
            let val: Value =
                serde_json::from_str(&normalized).map_err(|e| IngestError::InvalidJson {
                    source_label: opts.source_label.clone(),
                    reason: e.to_string(),
                })?;
            let objects: Vec<&Value> = match &val {
                Value::Object(_) => vec![&val],
                Value::Array(arr) => arr.iter().collect(),
                _ => {
                    return Err(IngestError::InvalidJson {
                        source_label: opts.source_label.clone(),
                        reason: "top-level JSON must be an object or array of objects".to_owned(),
                    });
                }
            };
            convert_json_records(&objects, stem_hint, &dest, format, opts)
        }
        IngestFormat::Jsonl => {
            let mut parsed = Vec::new();
            for (idx, line) in normalized.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let val: Value =
                    serde_json::from_str(trimmed).map_err(|e| IngestError::InvalidJson {
                        source_label: format!("{}:{}", opts.source_label, idx + 1),
                        reason: e.to_string(),
                    })?;
                parsed.push(val);
            }
            let refs: Vec<&Value> = parsed.iter().collect();
            convert_json_records(&refs, stem_hint, &dest, format, opts)
        }
    }
}

/// Evaluate a single [`IngestDraft`] against workspace schemas, links, duplicates, and quality rules.
///
/// `batch_locators` lists all target locators in the current ingest batch so mutually linked
/// notes imported together resolve cleanly without false broken-link rejections.
#[must_use]
pub fn evaluate_ingest_draft(
    snapshot: &VaultSnapshot,
    draft: &IngestDraft,
    batch_locators: &BTreeSet<Locator>,
    opts: &IngestGateOptions,
) -> IngestGateReport {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut unresolved_links = Vec::new();
    let mut ambiguous_links = Vec::new();
    let mut schema_diagnostics = Vec::new();
    let mut near_duplicates = Vec::new();
    let mut stale_description = false;

    let exists = snapshot.files.contains_key(&draft.locator);
    let action = if exists {
        if opts.refresh_existing {
            "refresh".to_owned()
        } else {
            errors.push(format!(
                "target '{}' already exists (pass --refresh to overwrite via CAS)",
                draft.locator.path
            ));
            "conflict".to_owned()
        }
    } else {
        "create".to_owned()
    };

    // 1. Parse document strictly.
    let parsed = match parse_document(draft.content.as_bytes()) {
        Ok(doc) => doc,
        Err(err) => {
            errors.push(format!("parse error: {err}"));
            return IngestGateReport {
                locator: draft.locator.clone(),
                source: draft.source_label.clone(),
                format: draft.format,
                admitted: false,
                action,
                errors,
                warnings,
                schema_diagnostics,
                unresolved_links,
                ambiguous_links,
                near_duplicates,
                stale_description,
            };
        }
    };

    // 2. Schema validation against workspace *.type.yaml registry.
    schema_diagnostics =
        snapshot
            .registry
            .validate_document(&draft.locator.repo, &draft.locator.path, &parsed);
    for diag in &schema_diagnostics {
        match diag.severity {
            Severity::Error => errors.push(format!("schema/{}: {}", diag.code, diag.message)),
            Severity::Warning | Severity::Drift => {
                warnings.push(format!("schema/{}: {}", diag.code, diag.message));
            }
        }
    }

    // 3. Outgoing [[wikilink]] validation against merged post-batch (snapshot + batch) index
    //    with the exact same exact-path and unique-stem precedence as `VaultSnapshot::resolve_link`.
    let mut merged_paths = snapshot.path_index.clone();
    let mut merged_stems = snapshot.stem_index.clone();
    for loc in batch_locators {
        merged_paths.insert((loc.repo.clone(), loc.path.clone()), loc.clone());
        if let Some(no_ext) = loc.path.strip_suffix(".md") {
            merged_paths.insert((loc.repo.clone(), no_ext.to_owned()), loc.clone());
        }
        let fname = loc.path.rsplit('/').next().unwrap_or(&loc.path);
        if let Some(stem) = fname.strip_suffix(".md") {
            let entry = merged_stems
                .entry((loc.repo.clone(), stem.to_owned()))
                .or_default();
            if !entry.contains(loc) {
                entry.push(loc.clone());
                entry.sort();
            }
        }
    }

    for link in &parsed.wikilinks {
        let target_repo = link.repo.as_deref().unwrap_or(&draft.locator.repo);
        let resolved = if !snapshot.members.contains_key(target_repo) {
            LinkResolution::Unresolved
        } else if let Some(loc) = merged_paths.get(&(target_repo.to_owned(), link.target.clone())) {
            LinkResolution::Resolved(loc.clone())
        } else if let Some(matches) =
            merged_stems.get(&(target_repo.to_owned(), link.target.clone()))
        {
            match matches.len() {
                1 => LinkResolution::Resolved(matches[0].clone()),
                n if n > 1 => LinkResolution::Ambiguous(matches.clone()),
                _ => LinkResolution::Unresolved,
            }
        } else {
            LinkResolution::Unresolved
        };

        let display_link = match &link.repo {
            Some(r) => format!("[[{}::{r}]]", link.target),
            None => format!("[[{}]]", link.target),
        };
        match resolved {
            LinkResolution::Resolved(_) => {}
            LinkResolution::Unresolved => {
                unresolved_links.push(display_link.clone());
                if opts.allow_unresolved_links {
                    warnings.push(format!("unresolved wikilink: {display_link}"));
                } else {
                    errors.push(format!("broken wikilink: {display_link}"));
                }
            }
            LinkResolution::Ambiguous(cands) => {
                let paths: Vec<&str> = cands.iter().map(|c| c.path.as_str()).collect();
                let msg = format!(
                    "ambiguous wikilink {display_link} -> [{}]",
                    paths.join(", ")
                );
                ambiguous_links.push(display_link);
                if opts.allow_ambiguous_links {
                    warnings.push(msg);
                } else {
                    errors.push(msg);
                }
            }
        }
    }

    // 4. Stale / weak description lint.
    let body_slice = draft.content.get(parsed.body_offset..).unwrap_or("");
    let description = parsed
        .frontmatter
        .as_ref()
        .and_then(|fm| match fm.get("description") {
            Some(FrontmatterValue::String(s)) => Some(s.trim()),
            _ => None,
        });
    if let Some(desc) = description {
        if is_stale_or_weak_description(desc, body_slice) {
            stale_description = true;
            warnings.push(format!(
                "stale_description: description '{}' is too short, placeholder-like, or lexically disconnected from body",
                truncate_chars(desc, 64)
            ));
        }
    } else {
        stale_description = true;
        warnings.push("missing frontmatter 'description' field".to_owned());
    }

    // 5. Near-duplicate check via workspace BM25 search.
    let probe = build_dedup_probe(body_slice, description.unwrap_or(""));
    if !probe.is_empty() && !snapshot.search.is_empty() {
        let small_corpus = snapshot.search.len() < 16;
        let hits = snapshot.search.query(&probe, false, Some(2), None, 4);
        for hit in hits {
            if hit.locator == draft.locator {
                continue;
            }
            if hit.matched_terms >= 3 && (!hit.weak || small_corpus) {
                near_duplicates.push(DuplicateCandidate {
                    locator: hit.locator,
                    score: hit.score,
                    matched_terms: hit.matched_terms,
                });
            }
        }
        if let Some(threshold) = opts.dedup_score_threshold
            && let Some(top) = near_duplicates.first()
            && top.score >= threshold
        {
            errors.push(format!(
                "near_duplicate: '{}' matches existing '{}' (score={:.2} >= {:.2})",
                draft.locator.path, top.locator.path, top.score, threshold
            ));
        }
    }

    IngestGateReport {
        locator: draft.locator.clone(),
        source: draft.source_label.clone(),
        format: draft.format,
        admitted: errors.is_empty(),
        action,
        errors,
        warnings,
        schema_diagnostics,
        unresolved_links,
        ambiguous_links,
        near_duplicates,
        stale_description,
    }
}

fn normalize_dest_dir(raw: &str) -> Result<String, IngestError> {
    let trimmed = raw.trim().replace('\\', "/");
    let cleaned = trimmed.trim_matches('/');
    if cleaned.is_empty() {
        return Ok(String::new());
    }
    for seg in cleaned.split('/') {
        if seg.is_empty()
            || seg == "."
            || seg == ".."
            || seg == ".git"
            || seg == ".2brain"
            || seg.contains('\0')
        {
            return Err(IngestError::InvalidDestination(raw.to_owned()));
        }
    }
    Ok(cleaned.to_owned())
}

fn build_target_locator(
    repo: &str,
    dest_dir: &str,
    stem_hint: &str,
) -> Result<Locator, IngestError> {
    let slug = slugify(stem_hint);
    let rel = if dest_dir.is_empty() {
        format!("{slug}.md")
    } else {
        format!("{dest_dir}/{slug}.md")
    };
    Locator::new(repo, &rel).map_err(|e| IngestError::InvalidLocator {
        path: rel,
        reason: e.to_string(),
    })
}

/// Convert an arbitrary filename/title into a safe lowercase kebab-case slug.
#[must_use]
pub fn slugify(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(96));
    let mut prev_dash = true;
    for ch in raw.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 96 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "note".to_owned()
    } else {
        out
    }
}

fn humanize_stem(stem: &str) -> String {
    let words: Vec<&str> = stem
        .split(['-', '_', ' '])
        .filter(|s| !s.is_empty())
        .collect();
    if words.is_empty() {
        "Untitled".to_owned()
    } else {
        words.join(" ")
    }
}

fn extract_plain_title(text: &str, stem_hint: &str) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let cleaned = trimmed.trim_start_matches('#').trim();
        if !cleaned.is_empty() && cleaned.chars().count() <= 100 {
            return cleaned.to_owned();
        }
        break;
    }
    humanize_stem(stem_hint)
}

fn synthesize_description(body: &str, title: &str) -> String {
    let mut para = String::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !para.is_empty() {
                break;
            }
            continue;
        }
        if trimmed.starts_with('#')
            || trimmed.starts_with("```")
            || trimmed.starts_with("---")
            || trimmed.starts_with('|')
        {
            continue;
        }
        let stripped = trimmed.trim_start_matches(['-', '*', '>', ' ']).trim();
        if stripped.is_empty() {
            continue;
        }
        if !para.is_empty() {
            para.push(' ');
        }
        para.push_str(stripped);
        if para.chars().count() >= 180 {
            break;
        }
    }
    let chosen = if para.chars().count() >= 16 {
        para
    } else {
        format!("Imported note covering {title}.")
    };
    truncate_chars(&chosen, 180)
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

fn insert_common_frontmatter(
    fm: &mut BTreeMap<String, Value>,
    opts: &ConvertOptions,
    description: &str,
) {
    if let Some(ref ty) = opts.default_type {
        fm.insert("type".to_owned(), Value::String(ty.clone()));
    }
    fm.insert(
        "description".to_owned(),
        Value::String(description.to_owned()),
    );
    if !opts.source_label.is_empty() {
        fm.insert(
            "source".to_owned(),
            Value::String(opts.source_label.clone()),
        );
    }
    if let Some(ref date) = opts.captured_date {
        fm.insert("captured".to_owned(), Value::String(date.clone()));
    }
}

fn render_note(fm: &BTreeMap<String, Value>, body: &str) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    // Render `type` and `description` first for readability, then remaining keys sorted.
    for key in ["type", "description"] {
        if let Some(val) = fm.get(key) {
            write_yaml_field(&mut out, key, val);
        }
    }
    for (key, val) in fm {
        if key == "type" || key == "description" {
            continue;
        }
        write_yaml_field(&mut out, key, val);
    }
    out.push_str("---\n\n");
    out.push_str(body.trim());
    out.push('\n');
    out
}

fn write_yaml_field(out: &mut String, key: &str, val: &Value) {
    // YAML 1.2 is a strict superset of JSON; JSON-encoded scalars/arrays prevent injection or
    // malformed YAML on colons, `#`, newlines, or leading `---`.
    let encoded = serde_json::to_string(val).unwrap_or_else(|_| "\"\"".to_owned());
    out.push_str(key);
    out.push_str(": ");
    out.push_str(&encoded);
    out.push('\n');
}

fn normalize_markdown(
    text: &str,
    stem_hint: &str,
    opts: &ConvertOptions,
) -> Result<String, IngestError> {
    let parsed = parse_document(text.as_bytes()).map_err(|_| IngestError::InvalidUtf8 {
        source_label: opts.source_label.clone(),
    })?;
    if let Some(ref fm_err) = parsed.frontmatter_error {
        return Err(IngestError::InvalidFrontmatter {
            source_label: opts.source_label.clone(),
            reason: fm_err.clone(),
        });
    }
    if let Some(ref fm) = parsed.frontmatter {
        let body = text.get(parsed.body_offset..).unwrap_or("").trim();
        let has_type = fm.contains_key("type");
        let has_desc = fm.contains_key("description");
        if (has_type || opts.default_type.is_none()) && has_desc {
            let mut normalized = text.trim_end().to_owned();
            normalized.push('\n');
            return Ok(normalized);
        }
        // Inject missing `type` / `description` at the top of existing frontmatter without
        // disturbing user fields.
        let mut injected = String::from("---\n");
        if !has_type && let Some(ref ty) = opts.default_type {
            write_yaml_field(&mut injected, "type", &Value::String(ty.clone()));
        }
        if !has_desc {
            let title = extract_plain_title(body, stem_hint);
            let desc = synthesize_description(body, &title);
            write_yaml_field(&mut injected, "description", &Value::String(desc));
        }
        // Strip the opening `---\n` from original text and splice injected keys right after it.
        if let Some(rest) = text.strip_prefix("---\n") {
            injected.push_str(rest.trim_end());
            injected.push('\n');
            return Ok(injected);
        }
    }

    let title = extract_plain_title(text, stem_hint);
    let desc = synthesize_description(text, &title);
    let mut fm = BTreeMap::new();
    insert_common_frontmatter(&mut fm, opts, &desc);
    Ok(render_note(&fm, text))
}

fn convert_json_records(
    records: &[&Value],
    stem_hint: &str,
    dest_dir: &str,
    format: IngestFormat,
    opts: &ConvertOptions,
) -> Result<Vec<IngestDraft>, IngestError> {
    if records.is_empty() {
        return Err(IngestError::EmptySource {
            source_label: opts.source_label.clone(),
        });
    }
    let mut drafts = Vec::with_capacity(records.len());
    let mut used_paths: HashSet<String> = HashSet::new();

    for (idx, val) in records.iter().enumerate() {
        let Some(obj) = val.as_object() else {
            return Err(IngestError::InvalidJson {
                source_label: format!("{}[{}]", opts.source_label, idx),
                reason: "each record must be a JSON object".to_owned(),
            });
        };
        let title = obj
            .get("title")
            .or_else(|| obj.get("name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if records.len() == 1 {
                    humanize_stem(stem_hint)
                } else {
                    format!("{} {}", humanize_stem(stem_hint), idx + 1)
                }
            });
        let raw_body = obj
            .get("body")
            .or_else(|| obj.get("content"))
            .or_else(|| obj.get("text"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| serde_json::to_string_pretty(val).unwrap_or_default());
        let desc = obj
            .get("description")
            .or_else(|| obj.get("summary"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| synthesize_description(&raw_body, &title));

        let mut fm = BTreeMap::new();
        if let Some(ty) = obj
            .get("type")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            fm.insert("type".to_owned(), Value::String(ty.trim().to_owned()));
        } else if let Some(ref ty) = opts.default_type {
            fm.insert("type".to_owned(), Value::String(ty.clone()));
        }
        fm.insert("description".to_owned(), Value::String(desc));
        if let Some(src) = obj
            .get("source")
            .or_else(|| obj.get("url"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            fm.insert("source".to_owned(), Value::String(src.trim().to_owned()));
        } else if !opts.source_label.is_empty() {
            fm.insert(
                "source".to_owned(),
                Value::String(opts.source_label.clone()),
            );
        }
        if let Some(date) = obj
            .get("date")
            .or_else(|| obj.get("captured"))
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
        {
            fm.insert("captured".to_owned(), Value::String(date.trim().to_owned()));
        } else if let Some(ref date) = opts.captured_date {
            fm.insert("captured".to_owned(), Value::String(date.clone()));
        }
        if let Some(tags) = obj.get("tags").and_then(Value::as_array) {
            let clean: Vec<Value> = tags
                .iter()
                .filter_map(Value::as_str)
                .map(|s| Value::String(s.trim().to_owned()))
                .collect();
            if !clean.is_empty() {
                fm.insert("tags".to_owned(), Value::Array(clean));
            }
        }

        let mut body = if raw_body.starts_with("# ") {
            raw_body
        } else {
            format!("# {title}\n\n{raw_body}")
        };
        if let Some(links) = obj.get("links").and_then(Value::as_array) {
            let clean_links: Vec<&str> = links
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            if !clean_links.is_empty() {
                body.push_str("\n\n## References\n\n");
                for target in clean_links {
                    let rendered = if target.starts_with("[[") && target.ends_with("]]") {
                        target.to_owned()
                    } else {
                        format!("[[{target}]]")
                    };
                    body.push_str("- ");
                    body.push_str(&rendered);
                    body.push('\n');
                }
            }
        }
        // Preserve any additional JSON fields so structured imports never silently drop data.
        const CONSUMED_KEYS: &[&str] = &[
            "title",
            "name",
            "body",
            "content",
            "text",
            "description",
            "summary",
            "type",
            "source",
            "url",
            "date",
            "captured",
            "tags",
            "links",
            "slug",
        ];
        let mut extra: BTreeMap<&str, &Value> = BTreeMap::new();
        for (k, v) in obj {
            if !CONSUMED_KEYS.contains(&k.as_str()) {
                extra.insert(k.as_str(), v);
            }
        }
        if !extra.is_empty() {
            let rendered_extra = serde_json::to_string_pretty(&extra).unwrap_or_default();
            body.push_str("\n\n## Additional Fields\n\n```json\n");
            body.push_str(&rendered_extra);
            body.push_str("\n```\n");
        }

        let slug_base = obj
            .get("slug")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(&title);
        let mut candidate = build_target_locator(&opts.repo, dest_dir, slug_base)?;
        if !used_paths.insert(candidate.path.clone()) {
            candidate =
                build_target_locator(&opts.repo, dest_dir, &format!("{slug_base}-{}", idx + 1))?;
            used_paths.insert(candidate.path.clone());
        }

        let content = render_note(&fm, &body);
        drafts.push(IngestDraft {
            locator: candidate,
            source_label: if records.len() == 1 {
                opts.source_label.clone()
            } else {
                format!("{}[{}]", opts.source_label, idx)
            },
            format,
            content,
        });
    }
    Ok(drafts)
}

/// Clean-room pure-Rust HTML → Markdown converter (strips scripts/styles/comments, safe links only).
fn html_to_markdown(html: &str) -> (Option<String>, String) {
    let mut title: Option<String> = None;
    let mut out = String::with_capacity(html.len() / 2);
    let bytes = html.as_bytes();
    let mut i = 0usize;
    let mut skip_tag: Option<&'static str> = None;
    let mut in_title = false;
    let mut title_buf = String::new();
    let mut in_pre = false;
    let mut active_href: Option<String> = None;
    let mut link_text = String::new();

    while i < bytes.len() {
        if bytes[i] == b'<' {
            // HTML comment <!-- ... -->
            if html[i..].starts_with("<!--") {
                if let Some(end) = html[i + 4..].find("-->") {
                    i += 4 + end + 3;
                    continue;
                }
                break;
            }
            let Some(rel_end) = html[i + 1..].find('>') else {
                break;
            };
            let tag_raw = &html[i + 1..i + 1 + rel_end];
            i += rel_end + 2;

            let trimmed = tag_raw.trim();
            let is_closing = trimmed.starts_with('/');
            let inner = trimmed.trim_start_matches('/').trim_end_matches('/').trim();
            let tag_name = inner
                .split(|c: char| c.is_ascii_whitespace() || c == '/')
                .next()
                .unwrap_or("")
                .to_ascii_lowercase();

            if let Some(skip) = skip_tag {
                if is_closing && tag_name == skip {
                    skip_tag = None;
                }
                continue;
            }

            match (is_closing, tag_name.as_str()) {
                (false, "script") => skip_tag = Some("script"),
                (false, "style") => skip_tag = Some("style"),
                (false, "noscript") => skip_tag = Some("noscript"),
                (false, "title") => {
                    in_title = true;
                    title_buf.clear();
                }
                (true, "title") => {
                    in_title = false;
                    let decoded = decode_html_entities(title_buf.trim());
                    if !decoded.is_empty() && title.is_none() {
                        title = Some(decoded);
                    }
                }
                (false, "h1") => out.push_str("\n\n# "),
                (false, "h2") => out.push_str("\n\n## "),
                (false, "h3") => out.push_str("\n\n### "),
                (false, "h4") => out.push_str("\n\n#### "),
                (false, "h5") => out.push_str("\n\n##### "),
                (false, "h6") => out.push_str("\n\n###### "),
                (
                    true,
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "section" | "article"
                    | "blockquote" | "tr",
                ) => {
                    out.push_str("\n\n");
                }
                (false, "br") => out.push('\n'),
                (false, "li") => out.push_str("\n- "),
                (false, "pre") => {
                    in_pre = true;
                    out.push_str("\n\n```\n");
                }
                (true, "pre") => {
                    in_pre = false;
                    out.push_str("\n```\n\n");
                }
                (false, "code") if !in_pre => out.push('`'),
                (true, "code") if !in_pre => out.push('`'),
                (false, "a") => {
                    active_href = extract_safe_href(inner);
                    link_text.clear();
                }
                (true, "a") => {
                    let label = escape_markdown_link_label(&decode_html_entities(link_text.trim()));
                    if let Some(href) = active_href.take() {
                        let safe_url = encode_markdown_link_dest(&href);
                        if label.is_empty() {
                            out.push_str(&safe_url);
                        } else {
                            out.push('[');
                            out.push_str(&label);
                            out.push_str("](");
                            out.push_str(&safe_url);
                            out.push(')');
                        }
                    } else {
                        out.push_str(&label);
                    }
                    link_text.clear();
                }
                _ => {}
            }
            continue;
        }

        let next_lt = html[i..].find('<').map_or(html.len(), |p| i + p);
        let chunk = &html[i..next_lt];
        i = next_lt;
        if skip_tag.is_some() {
            continue;
        }
        if in_title {
            title_buf.push_str(chunk);
        } else if active_href.is_some() {
            link_text.push_str(chunk);
        } else if in_pre {
            out.push_str(&decode_html_entities(chunk));
        } else {
            // Re-escape angle brackets after decoding HTML entities so `&lt;img onerror=...&gt;`
            // can never reintroduce executable raw HTML into rendered Markdown.
            let decoded = escape_raw_html_in_markdown(&decode_html_entities(chunk));
            for word in decoded.split_whitespace() {
                if !out.is_empty() && !out.ends_with(['\n', ' ', '`', '[']) {
                    out.push(' ');
                }
                out.push_str(word);
            }
        }
    }

    // Collapse 3+ consecutive newlines down to 2.
    let mut compact = String::with_capacity(out.len());
    let mut blank_run = 0usize;
    for line in out.lines() {
        let trimmed = if in_pre { line } else { line.trim_end() };
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 && !compact.is_empty() {
                compact.push('\n');
            }
        } else {
            blank_run = 0;
            compact.push_str(trimmed);
            compact.push('\n');
        }
    }
    (title, compact.trim().to_owned())
}

fn extract_safe_href(tag_inner: &str) -> Option<String> {
    let lower = tag_inner.to_ascii_lowercase();
    let idx = lower.find("href=")?;
    let rest = tag_inner[idx + 5..].trim_start();
    let raw = if let Some(after_q) = rest.strip_prefix('"') {
        after_q.split('"').next()?
    } else if let Some(after_q) = rest.strip_prefix('\'') {
        after_q.split('\'').next()?
    } else {
        rest.split_whitespace().next()?
    };
    let decoded = decode_html_entities(raw.trim());
    let check = decoded.to_ascii_lowercase();
    if check.starts_with("http://") || check.starts_with("https://") || check.starts_with("mailto:")
    {
        Some(decoded)
    } else {
        None
    }
}

fn decode_html_entities(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        if let Some(semi) = rest.find(';')
            && semi <= 10
        {
            let entity = &rest[1..semi];
            let replacement = match entity {
                "amp" => Some("&".to_owned()),
                "lt" => Some("<".to_owned()),
                "gt" => Some(">".to_owned()),
                "quot" => Some("\"".to_owned()),
                "apos" | "#39" => Some("'".to_owned()),
                "nbsp" => Some(" ".to_owned()),
                s if s.starts_with("#x") || s.starts_with("#X") => u32::from_str_radix(&s[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string()),
                s if s.starts_with('#') => s[1..]
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string()),
                _ => None,
            };
            if let Some(ch) = replacement {
                out.push_str(&ch);
                rest = &rest[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &rest[1..];
    }
    out.push_str(rest);
    out
}

fn escape_raw_html_in_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(ch),
        }
    }
    out
}

fn escape_markdown_link_label(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '[' => out.push_str("\\["),
            ']' => out.push_str("\\]"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\n' | '\r' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

fn encode_markdown_link_dest(url: &str) -> String {
    let mut out = String::with_capacity(url.len());
    for ch in url.chars() {
        match ch {
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            ' ' | '\t' | '\n' | '\r' => out.push_str("%20"),
            _ => out.push(ch),
        }
    }
    out
}

/// Strict RFC 4180 CSV parser: preserves exact cell whitespace and intentional empty records,
/// and rejects unclosed or misplaced quotes instead of silently distorting data.
fn parse_csv(input: &str, source_label: &str) -> Result<Vec<Vec<String>>, IngestError> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut just_closed_quote = false;
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    let _ = chars.next();
                    cell.push('"');
                } else {
                    in_quotes = false;
                    just_closed_quote = true;
                }
            } else {
                cell.push(ch);
            }
        } else if just_closed_quote {
            match ch {
                ',' => {
                    row.push(std::mem::take(&mut cell));
                    just_closed_quote = false;
                }
                '\n' => {
                    row.push(std::mem::take(&mut cell));
                    rows.push(std::mem::take(&mut row));
                    just_closed_quote = false;
                }
                other => {
                    return Err(IngestError::InvalidCsv {
                        source_label: source_label.to_owned(),
                        reason: format!("unexpected character '{other}' after closing quote"),
                    });
                }
            }
        } else {
            match ch {
                '"' => {
                    if cell.is_empty() {
                        in_quotes = true;
                    } else {
                        return Err(IngestError::InvalidCsv {
                            source_label: source_label.to_owned(),
                            reason: "unescaped quote inside unquoted CSV field".to_owned(),
                        });
                    }
                }
                ',' => {
                    row.push(std::mem::take(&mut cell));
                }
                '\n' => {
                    row.push(std::mem::take(&mut cell));
                    rows.push(std::mem::take(&mut row));
                }
                other => cell.push(other),
            }
        }
    }
    if in_quotes {
        return Err(IngestError::InvalidCsv {
            source_label: source_label.to_owned(),
            reason: "unclosed quoted CSV field at end of input".to_owned(),
        });
    }
    if !cell.is_empty() || !row.is_empty() || just_closed_quote {
        row.push(cell);
        rows.push(row);
    }
    Ok(rows)
}

fn render_csv_markdown_table(rows: &[Vec<String>]) -> String {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }
    let escape = |s: &str| s.replace('|', "\\|").replace('\n', " ");
    let mut out = String::new();
    let header = &rows[0];
    out.push('|');
    for c in 0..cols {
        out.push(' ');
        out.push_str(&escape(header.get(c).map_or("", String::as_str)));
        out.push_str(" |");
    }
    out.push('\n');
    out.push('|');
    for _ in 0..cols {
        out.push_str(" --- |");
    }
    out.push('\n');
    for row in rows.iter().skip(1) {
        out.push('|');
        for c in 0..cols {
            out.push(' ');
            out.push_str(&escape(row.get(c).map_or("", String::as_str)));
            out.push_str(" |");
        }
        out.push('\n');
    }
    out
}

fn is_weak_description(desc: &str) -> bool {
    let trimmed = desc.trim();
    if trimmed.chars().count() < 16 {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("todo")
        || lower.starts_with("tbd")
        || lower.starts_with("wip")
        || lower.contains("lorem ipsum")
    {
        return true;
    }
    !tokenize(trimmed)
        .into_iter()
        .any(|t| t.term.chars().count() >= 4)
}

fn is_lexically_disconnected(desc: &str, body: &str) -> bool {
    let desc_terms: HashSet<String> = tokenize(desc.trim())
        .into_iter()
        .map(|t| t.term)
        .filter(|t| t.chars().count() >= 4)
        .collect();
    if desc_terms.is_empty() {
        return true;
    }
    let body_terms: HashSet<String> = tokenize(body)
        .into_iter()
        .map(|t| t.term)
        .filter(|t| t.chars().count() >= 4)
        .collect();
    if body_terms.is_empty() {
        return false;
    }
    desc_terms.is_disjoint(&body_terms)
}

fn is_stale_or_weak_description(desc: &str, body: &str) -> bool {
    is_weak_description(desc) || is_lexically_disconnected(desc, body)
}

fn build_dedup_probe(body: &str, description: &str) -> String {
    let mut combined = String::with_capacity(description.len() + 384);
    for ch in body.chars().take(400) {
        combined.push(ch);
    }
    combined.push(' ');
    combined.push_str(description);
    distinct_terms(&combined)
        .into_iter()
        .filter(|t| t.chars().count() >= 4)
        .take(8)
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IndexedFile, MemberMeta};
    use tb_contracts::RevisionDigest;
    use tb_graph::TypedLinkGraph;
    use tb_schema::SchemaRegistry;
    use tb_search::SearchIndex;

    fn sample_snapshot() -> VaultSnapshot {
        let mut members = BTreeMap::new();
        members.insert(
            "vault".to_owned(),
            MemberMeta {
                repo: "vault".to_owned(),
                root: "/tmp/vault".to_owned(),
                role: "editable".to_owned(),
                deps: Vec::new(),
            },
        );
        let existing_loc = Locator::new("vault", "notes/existing.md").unwrap();
        let raw = "---\ndescription: \"Quantum lattice architecture overview\"\n---\n\n# Existing\n\nQuantum lattice hexagonal architecture notes.\n";
        let doc = parse_document(raw.as_bytes()).unwrap();
        let file = IndexedFile {
            locator: existing_loc.clone(),
            sha256: RevisionDigest::of_bytes(raw.as_bytes()),
            short_hash: "1122334455667788".to_owned(),
            size_bytes: raw.len(),
            raw_text: raw.to_owned(),
            doc,
            description: Some("Quantum lattice architecture overview".to_owned()),
            declared_type: None,
        };
        let mut files = BTreeMap::new();
        files.insert(existing_loc.clone(), file);
        let mut path_index = BTreeMap::new();
        path_index.insert(
            ("vault".to_owned(), "notes/existing.md".to_owned()),
            existing_loc.clone(),
        );
        path_index.insert(
            ("vault".to_owned(), "notes/existing".to_owned()),
            existing_loc.clone(),
        );
        let mut stem_index = BTreeMap::new();
        stem_index.insert(
            ("vault".to_owned(), "existing".to_owned()),
            vec![existing_loc.clone()],
        );
        let search = SearchIndex::build([(
            existing_loc.clone(),
            "Quantum lattice hexagonal architecture notes.",
            "Quantum lattice architecture overview",
        )]);
        VaultSnapshot {
            generation: VaultSnapshot::empty().generation,
            members,
            files,
            registry: SchemaRegistry::default(),
            graph: TypedLinkGraph::build(vec![existing_loc], &[]),
            search,
            diagnostics: Vec::new(),
            path_index,
            stem_index,
            indexed_symlinks: BTreeSet::new(),
        }
    }

    #[test]
    fn html_csv_and_jsonl_converters_produce_valid_typed_markdown() {
        let opts = ConvertOptions {
            repo: "vault".to_owned(),
            dest_dir: "inbox".to_owned(),
            default_type: Some("note".to_owned()),
            captured_date: Some("2026-11-28".to_owned()),
            source_label: "sample.html".to_owned(),
        };
        let html = b"<html><head><title>Graph &amp; Search</title><script>alert(1)</script></head><body><h1>Graph &amp; Search</h1><p>Read <a href=\"https://example.com\">docs</a> and <a href=\"javascript:evil()\">skip</a>.</p></body></html>";
        let html_drafts = convert_source("sample", IngestFormat::Html, html, &opts).unwrap();
        assert_eq!(html_drafts.len(), 1);
        assert!(
            html_drafts[0]
                .content
                .contains("[docs](https://example.com)")
        );
        assert!(!html_drafts[0].content.contains("javascript:"));
        assert!(!html_drafts[0].content.contains("alert(1)"));

        let csv = b"name,role\n\"Alice, MD\",lead\nBob,dev\n";
        let csv_drafts = convert_source("team", IngestFormat::Csv, csv, &opts).unwrap();
        assert!(csv_drafts[0].content.contains("| Alice, MD | lead |"));

        let jsonl = b"{\"title\":\"Alpha Note\",\"body\":\"Body referencing [[Beta Note]]\"}\n{\"title\":\"Beta Note\",\"body\":\"Companion note\"}\n";
        let jsonl_drafts = convert_source("batch", IngestFormat::Jsonl, jsonl, &opts).unwrap();
        assert_eq!(jsonl_drafts.len(), 2);
        assert_eq!(jsonl_drafts[0].locator.path, "inbox/alpha-note.md");
        assert_eq!(jsonl_drafts[1].locator.path, "inbox/beta-note.md");
    }

    #[test]
    fn ingest_gate_blocks_broken_links_but_allows_valid_and_intrabatch_links() {
        let snap = sample_snapshot();
        let opts = ConvertOptions {
            repo: "vault".to_owned(),
            dest_dir: "inbox".to_owned(),
            default_type: None,
            captured_date: None,
            source_label: "draft.md".to_owned(),
        };

        let broken = convert_source(
            "broken",
            IngestFormat::Markdown,
            b"---\ndescription: \"Draft linking to missing target\"\n---\n\n# Broken\n\nSee [[nonexistent-target]] and [[existing]].\n",
            &opts,
        )
        .unwrap();
        let batch: BTreeSet<Locator> = broken.iter().map(|d| d.locator.clone()).collect();
        let rep = evaluate_ingest_draft(&snap, &broken[0], &batch, &IngestGateOptions::default());
        assert!(!rep.admitted, "broken wikilink must fail closed");
        assert_eq!(rep.unresolved_links, vec!["[[nonexistent-target]]"]);

        // Now link to another note inside the same ingest batch -> must resolve and admit.
        let a = convert_source(
            "alpha",
            IngestFormat::Markdown,
            b"---\ndescription: \"Alpha note linking to beta peer and existing\"\n---\n\n# Alpha\n\nLinks [[beta]] and [[existing]].\n",
            &opts,
        )
        .unwrap();
        let b = convert_source(
            "beta",
            IngestFormat::Markdown,
            b"---\ndescription: \"Beta peer note linked by alpha\"\n---\n\n# Beta\n\nLinked by [[alpha]].\n",
            &opts,
        )
        .unwrap();
        let mut pair_batch = BTreeSet::new();
        pair_batch.insert(a[0].locator.clone());
        pair_batch.insert(b[0].locator.clone());
        let rep_a = evaluate_ingest_draft(&snap, &a[0], &pair_batch, &IngestGateOptions::default());
        assert!(
            rep_a.admitted,
            "intra-batch links must resolve: {:?}",
            rep_a.errors
        );
    }

    #[test]
    fn ingest_gate_flags_stale_description_and_near_duplicates() {
        let snap = sample_snapshot();
        let opts = ConvertOptions {
            repo: "vault".to_owned(),
            dest_dir: "inbox".to_owned(),
            default_type: None,
            captured_date: None,
            source_label: "dup.md".to_owned(),
        };
        let dup = convert_source(
            "dup",
            IngestFormat::Markdown,
            b"---\ndescription: \"Completely unrelated bananas pineapple\"\n---\n\n# Quantum Lattice\n\nQuantum lattice hexagonal architecture notes and overview.\n",
            &opts,
        )
        .unwrap();
        let batch: BTreeSet<Locator> = dup.iter().map(|d| d.locator.clone()).collect();
        let rep = evaluate_ingest_draft(&snap, &dup[0], &batch, &IngestGateOptions::default());
        assert!(
            rep.stale_description,
            "disconnected description must be flagged"
        );
        assert!(
            !rep.near_duplicates.is_empty(),
            "near duplicate against existing note must be surfaced"
        );
    }

    #[test]
    fn ingest_enforces_codex_invariants_frontmatter_ambiguity_json_csv_and_html_xss() {
        let snap = sample_snapshot();
        let opts = ConvertOptions {
            repo: "vault".to_owned(),
            dest_dir: "inbox".to_owned(),
            default_type: Some("note".to_owned()),
            captured_date: None,
            source_label: "adversarial".to_owned(),
        };

        // 1. Malformed frontmatter fails closed instead of being wrapped.
        let bad_fm = b"---\nkey: [unclosed\n---\n\n# Body\n";
        assert!(matches!(
            convert_source("bad", IngestFormat::Markdown, bad_fm, &opts),
            Err(IngestError::InvalidFrontmatter { .. })
        ));

        // 2. Post-batch stem ambiguity: existing `notes/existing.md` + new `inbox/existing.md`
        //    makes `[[existing]]` ambiguous and fails closed.
        let new_existing = Locator::new("vault", "inbox/existing.md").unwrap();
        let linker = convert_source(
            "linker",
            IngestFormat::Markdown,
            b"---\ndescription: \"Linker referencing ambiguous existing stem\"\n---\n\nSee [[existing]].\n",
            &opts,
        )
        .unwrap();
        let mut batch = BTreeSet::new();
        batch.insert(new_existing);
        batch.insert(linker[0].locator.clone());
        let rep = evaluate_ingest_draft(&snap, &linker[0], &batch, &IngestGateOptions::default());
        assert!(!rep.admitted, "post-batch stem ambiguity must fail closed");
        assert_eq!(rep.ambiguous_links, vec!["[[existing]]"]);

        // 3. JSON extra fields are preserved cleanly in Additional Fields.
        let json_doc =
            b"{\"title\":\"Record\",\"body\":\"Hello\",\"author\":\"Ada\",\"priority\":1}";
        let json_drafts = convert_source("rec", IngestFormat::Json, json_doc, &opts).unwrap();
        assert!(json_drafts[0].content.contains("\"author\": \"Ada\""));
        assert!(json_drafts[0].content.contains("\"priority\": 1"));

        // 4. CSV preserves whitespace & empty rows, and rejects malformed quotes.
        let csv_ok = b"a,b\n\" Alice \",1\n,\n";
        let csv_drafts = convert_source("rows", IngestFormat::Csv, csv_ok, &opts).unwrap();
        assert!(csv_drafts[0].content.contains("|  Alice  | 1 |"));
        assert!(csv_drafts[0].content.contains("|  |  |"));
        let csv_bad = b"a,b\n\"unclosed,1\n";
        assert!(matches!(
            convert_source("badcsv", IngestFormat::Csv, csv_bad, &opts),
            Err(IngestError::InvalidCsv { .. })
        ));

        // 5. HTML entity decoding never reintroduces raw executable HTML tags into Markdown.
        let html_xss =
            b"<html><body><p>&lt;img src=x onerror=alert(1)&gt;</p><a href=\"https://ex.com/(a)\">[x]</a></body></html>";
        let html_drafts = convert_source("xss", IngestFormat::Html, html_xss, &opts).unwrap();
        assert!(
            !html_drafts[0].content.contains("<img"),
            "decoded entity must stay escaped: {}",
            html_drafts[0].content
        );
        assert!(html_drafts[0].content.contains("&lt;img"));
        assert!(
            html_drafts[0]
                .content
                .contains("[\\[x\\]](https://ex.com/%28a%29)")
        );
    }

    #[test]
    fn audit_descriptions_classifies_ok_missing_weak_and_stale_notes() {
        let mut snap = sample_snapshot();
        let make_file = |path: &str, raw: &str, desc: Option<&str>| {
            let loc = Locator::new("vault", path).unwrap();
            let doc = parse_document(raw.as_bytes()).unwrap();
            (
                loc.clone(),
                IndexedFile {
                    locator: loc,
                    sha256: RevisionDigest::of_bytes(raw.as_bytes()),
                    short_hash: "aaaa1111bbbb2222".to_owned(),
                    size_bytes: raw.len(),
                    raw_text: raw.to_owned(),
                    doc,
                    description: desc.map(str::to_owned),
                    declared_type: None,
                },
            )
        };
        let (l1, f1) = make_file(
            "notes/missing.md",
            "# Missing\n\nSome content here.\n",
            None,
        );
        let (l2, f2) = make_file(
            "notes/weak.md",
            "# Weak\n\nDetailed architecture notes.\n",
            Some("TODO fix later"),
        );
        let (l3, f3) = make_file(
            "notes/stale.md",
            "# Rust Graph\n\nCompressed sparse row topology metrics.\n",
            Some("Completely unrelated bananas pineapple smoothie recipe"),
        );
        snap.files.insert(l1, f1);
        snap.files.insert(l2, f2);
        snap.files.insert(l3, f3);

        let report = audit_descriptions(&snap, None, 10);
        assert_eq!(report.summary.total, 4);
        assert_eq!(report.summary.ok, 1);
        assert_eq!(report.summary.missing, 1);
        assert_eq!(report.summary.weak, 1);
        assert_eq!(report.summary.stale, 1);
        assert_eq!(report.findings.len(), 3);
    }
}
