//! Offline note enrichment (`2brain enrich`, Backlog P1-6, 0 I/O).
//!
//! Pure application logic kept strictly outside the MCP serving path:
//! - Deterministic cache key: `(note_hash, model, prompt_version)` where `note_hash` is the
//!   SHA-256 hex digest of the normalized note body (frontmatter edits never invalidate the hash).
//! - Fail-closed JSONL cache parsing (never silently skips malformed lines).
//! - Safe frontmatter splicing that preserves existing fields and exact note body bytes, then
//!   verifies clean re-parse via `tb_syntax::parse_document`.
//! - Deterministic zero-API heuristic extractor for offline baselines, CI, and tests.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tb_contracts::{Locator, RevisionDigest};
use tb_search::{FreshnessMeta, SearchDocInput, SearchIndex};
use tb_syntax::{FrontmatterValue, parse_document, tokenize};

use crate::VaultSnapshot;
use crate::ingest::IngestWriteOp;

pub const ENRICH_CACHE_SCHEMA_VERSION: &str = "2brain.enrichment-cache/v1";
const MAX_KEYWORDS: usize = 16;
const MAX_SEARCH_QUESTIONS: usize = 8;
const MAX_ITEM_CHARS: usize = 200;

/// Compute the deterministic SHA-256 hex digest of a note's normalized body.
///
/// Frontmatter updates (`keywords`, `search_questions`) do not alter this hash, so applying
/// enrichment is idempotent and never triggers redundant LLM calls.
#[must_use]
pub fn body_content_sha256(body: &str) -> String {
    let normalized = body
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    RevisionDigest::of_bytes(normalized.trim().as_bytes()).to_hex()
}

/// Deterministic cache key: `(note_hash, model, prompt_version)`.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct EnrichmentCacheKey {
    pub note_hash: String,
    pub model: String,
    pub prompt_version: String,
}

/// One persisted offline enrichment entry in JSONL cache storage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EnrichmentRecord {
    pub schema_version: String,
    pub repo: String,
    pub path: String,
    pub note_hash: String,
    pub model: String,
    pub prompt_version: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub search_questions: Vec<String>,
}

impl EnrichmentRecord {
    #[must_use]
    pub fn key(&self) -> EnrichmentCacheKey {
        EnrichmentCacheKey {
            note_hash: self.note_hash.clone(),
            model: self.model.clone(),
            prompt_version: self.prompt_version.clone(),
        }
    }

    /// Normalize and validate enrichment lists (trim, dedup case-insensitively, cap counts).
    pub fn sanitize(&mut self) -> Result<(), String> {
        if self.schema_version != ENRICH_CACHE_SCHEMA_VERSION {
            return Err(format!(
                "unsupported enrichment schema_version '{}' (expected '{ENRICH_CACHE_SCHEMA_VERSION}')",
                self.schema_version
            ));
        }
        if self.note_hash.len() != 64 || !self.note_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "invalid note_hash '{}' (expected 64-char SHA-256 hex)",
                self.note_hash
            ));
        }
        if self.model.trim().is_empty() || self.prompt_version.trim().is_empty() {
            return Err("model and prompt_version must be non-empty".to_owned());
        }
        self.note_hash = self.note_hash.to_ascii_lowercase();
        self.model = self.model.trim().to_owned();
        self.prompt_version = self.prompt_version.trim().to_owned();
        self.keywords = sanitize_list(&self.keywords, MAX_KEYWORDS);
        self.search_questions = sanitize_list(&self.search_questions, MAX_SEARCH_QUESTIONS);
        Ok(())
    }
}

fn sanitize_list(items: &[String], max_items: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for raw in items {
        let cleaned = raw
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(MAX_ITEM_CHARS)
            .collect::<String>();
        let trimmed = cleaned.trim();
        if trimmed.is_empty() {
            continue;
        }
        if seen.insert(trimmed.to_ascii_lowercase()) {
            out.push(trimmed.to_owned());
            if out.len() >= max_items {
                break;
            }
        }
    }
    out
}

/// Deterministic in-memory view of an offline enrichment JSONL cache.
#[derive(Clone, Debug, Default)]
pub struct EnrichmentCache {
    records: HashMap<EnrichmentCacheKey, EnrichmentRecord>,
}

impl EnrichmentCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Fail-closed JSONL parser: rejects any malformed line or invalid record schema.
    pub fn from_jsonl(raw: &str) -> Result<Self, String> {
        let mut cache = Self::new();
        for (idx, line) in raw.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let mut rec: EnrichmentRecord = serde_json::from_str(trimmed)
                .map_err(|e| format!("enrichment cache line {}: invalid JSON: {e}", idx + 1))?;
            rec.sanitize()
                .map_err(|e| format!("enrichment cache line {}: {e}", idx + 1))?;
            cache.insert(rec);
        }
        Ok(cache)
    }

    pub fn insert(&mut self, record: EnrichmentRecord) {
        self.records.insert(record.key(), record);
    }

    #[must_use]
    pub fn get(
        &self,
        note_hash: &str,
        model: &str,
        prompt_version: &str,
    ) -> Option<&EnrichmentRecord> {
        let key = EnrichmentCacheKey {
            note_hash: note_hash.to_ascii_lowercase(),
            model: model.trim().to_owned(),
            prompt_version: prompt_version.trim().to_owned(),
        };
        self.records.get(&key)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Serialize records in deterministic key order as JSONL.
    #[must_use]
    pub fn to_jsonl(&self) -> String {
        let mut entries: Vec<&EnrichmentRecord> = self.records.values().collect();
        entries.sort_by(|a, b| {
            a.repo
                .cmp(&b.repo)
                .then_with(|| a.path.cmp(&b.path))
                .then_with(|| a.note_hash.cmp(&b.note_hash))
                .then_with(|| a.model.cmp(&b.model))
                .then_with(|| a.prompt_version.cmp(&b.prompt_version))
        });
        let mut out = String::new();
        for rec in entries {
            if let Ok(line) = serde_json::to_string(rec) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        out
    }
}

/// Status of one workspace note during offline enrichment planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Cached,
    Pending,
    AlreadyEnriched,
    EmptyBody,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrichmentWorkItem {
    pub repo: String,
    pub path: String,
    pub note_hash: String,
    pub model: String,
    pub prompt_version: String,
    pub title: String,
    pub description: Option<String>,
    pub body_excerpt: String,
    pub status: PlanStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnrichmentPlanReport {
    pub model: String,
    pub prompt_version: String,
    pub total_notes: usize,
    pub cached_hits: usize,
    pub pending_misses: usize,
    pub already_enriched: usize,
    pub empty_body: usize,
    pub items: Vec<EnrichmentWorkItem>,
}

/// Plan offline enrichment over a [`VaultSnapshot`], matching each note's body SHA-256 against
/// `(note_hash, model, prompt_version)` in `cache`.
#[must_use]
pub fn plan_enrichment(
    snapshot: &VaultSnapshot,
    cache: &EnrichmentCache,
    model: &str,
    prompt_version: &str,
    repo_filter: Option<&str>,
    only_missing: bool,
) -> EnrichmentPlanReport {
    let mut items = Vec::new();
    let mut cached_hits = 0usize;
    let mut pending_misses = 0usize;
    let mut already_enriched = 0usize;
    let mut empty_body = 0usize;

    for (loc, file) in &snapshot.files {
        if repo_filter.is_some_and(|r| loc.repo != r) {
            continue;
        }
        let body = file.body();
        if body.trim().is_empty() {
            empty_body += 1;
            continue;
        }
        let note_hash = body_content_sha256(body);
        let has_existing = has_enrichment_fields(file.doc.frontmatter.as_ref());
        let status = if only_missing && has_existing {
            already_enriched += 1;
            PlanStatus::AlreadyEnriched
        } else if cache.get(&note_hash, model, prompt_version).is_some() {
            cached_hits += 1;
            PlanStatus::Cached
        } else {
            pending_misses += 1;
            PlanStatus::Pending
        };

        let title = extract_note_title(body, &loc.path);
        let excerpt: String = body.trim().chars().take(1200).collect();
        items.push(EnrichmentWorkItem {
            repo: loc.repo.clone(),
            path: loc.path.clone(),
            note_hash,
            model: model.to_owned(),
            prompt_version: prompt_version.to_owned(),
            title,
            description: file.description.clone(),
            body_excerpt: excerpt,
            status,
        });
    }

    EnrichmentPlanReport {
        model: model.to_owned(),
        prompt_version: prompt_version.to_owned(),
        total_notes: cached_hits + pending_misses + already_enriched + empty_body,
        cached_hits,
        pending_misses,
        already_enriched,
        empty_body,
        items,
    }
}

fn has_enrichment_fields(fm: Option<&BTreeMap<String, FrontmatterValue>>) -> bool {
    let Some(fm) = fm else {
        return false;
    };
    let non_empty = |key: &str| match fm.get(key) {
        Some(FrontmatterValue::String(s)) => !s.trim().is_empty(),
        Some(FrontmatterValue::Seq(seq)) => !seq.is_empty(),
        _ => false,
    };
    non_empty("keywords") && non_empty("search_questions")
}

fn extract_note_title(body: &str, path: &str) -> String {
    for line in body.lines() {
        let t = line.trim();
        if let Some(h) = t.strip_prefix("# ")
            && !h.trim().is_empty()
        {
            return h.trim().to_owned();
        }
    }
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".md")
        .unwrap_or(file)
        .replace(['-', '_'], " ")
}

/// Deterministic zero-API heuristic extractor for `keywords` and `search_questions`.
///
/// Used for offline baselines, CI tests, and local pre-checks before paying for LLM calls.
#[must_use]
pub fn heuristic_enrich(
    title: &str,
    description: Option<&str>,
    body: &str,
) -> (Vec<String>, Vec<String>) {
    const STOPWORDS: &[&str] = &[
        "the", "and", "for", "with", "that", "this", "from", "are", "was", "were", "have", "has",
        "had", "not", "but", "you", "your", "can", "will", "into", "over", "under", "about",
        "when", "where", "what", "which", "who", "how", "why", "all", "any", "each", "more",
        "most", "some", "than", "then", "them", "they", "their", "there", "here", "only", "also",
        "very", "just", "such", "use", "used", "using", "should", "must", "may", "might", "could",
        "would", "true", "false", "none", "null", "http", "https", "www", "com", "org", "note",
        "notes", "file", "files", "see", "via", "per",
    ];

    let mut scores: BTreeMap<String, usize> = BTreeMap::new();
    let mut score_text = |text: &str, weight: usize| {
        for tok in tokenize(text) {
            if tok.term.chars().count() < 3 || tok.term.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            if STOPWORDS.contains(&tok.term.as_str()) {
                continue;
            }
            *scores.entry(tok.term).or_insert(0) += weight;
        }
    };

    score_text(title, 4);
    if let Some(desc) = description {
        score_text(desc, 3);
    }
    score_text(body, 1);

    let mut ranked: Vec<(String, usize)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let keywords: Vec<String> = ranked.into_iter().take(8).map(|(term, _)| term).collect();

    let mut questions = Vec::new();
    let clean_title = title.trim();
    if !clean_title.is_empty() {
        questions.push(format!("What is {clean_title} and how does it work?"));
    }
    if let Some(desc) = description.map(str::trim).filter(|s| !s.is_empty()) {
        let short_desc: String = desc.chars().take(110).collect();
        questions.push(format!("Which note explains: {short_desc}?"));
    }
    for line in body.lines() {
        if questions.len() >= 4 {
            break;
        }
        let trimmed = line.trim();
        if let Some(h2) = trimmed.strip_prefix("## ") {
            let h = h2.trim();
            if h.len() >= 4 && !h.eq_ignore_ascii_case("references") {
                questions.push(format!("How does {clean_title} handle {h}?"));
            }
        }
    }

    (
        sanitize_list(&keywords, MAX_KEYWORDS),
        sanitize_list(&questions, MAX_SEARCH_QUESTIONS),
    )
}

/// Splice `keywords` and `search_questions` into a note's YAML frontmatter without altering
/// unrelated frontmatter keys or the note body.
///
/// Fails closed if existing frontmatter is malformed or if the rewritten note does not parse cleanly.
pub fn apply_enrichment_to_note(
    raw_text: &str,
    keywords: &[String],
    search_questions: &[String],
) -> Result<String, String> {
    let clean_kw = sanitize_list(keywords, MAX_KEYWORDS);
    let clean_sq = sanitize_list(search_questions, MAX_SEARCH_QUESTIONS);
    if clean_kw.is_empty() && clean_sq.is_empty() {
        return Ok(raw_text.to_owned());
    }

    let normalized = raw_text.replace("\r\n", "\n").replace('\r', "\n");
    let parsed = parse_document(normalized.as_bytes()).map_err(|_| "invalid UTF-8".to_owned())?;
    if let Some(err) = parsed.frontmatter_error {
        return Err(format!("existing frontmatter is malformed: {err}"));
    }

    let before_body_hash = body_content_sha256(normalized.get(parsed.body_offset..).unwrap_or(""));

    let mut rendered_fields = String::new();
    let mut keys_to_replace: Vec<&str> = Vec::with_capacity(2);
    if !clean_kw.is_empty() {
        keys_to_replace.push("keywords");
        let kw_json = serde_json::to_string(&Value::Array(
            clean_kw.into_iter().map(Value::String).collect(),
        ))
        .map_err(|e| format!("failed to encode keywords: {e}"))?;
        rendered_fields.push_str("keywords: ");
        rendered_fields.push_str(&kw_json);
        rendered_fields.push('\n');
    }
    if !clean_sq.is_empty() {
        keys_to_replace.push("search_questions");
        let sq_json = serde_json::to_string(&Value::Array(
            clean_sq.into_iter().map(Value::String).collect(),
        ))
        .map_err(|e| format!("failed to encode search_questions: {e}"))?;
        rendered_fields.push_str("search_questions: ");
        rendered_fields.push_str(&sq_json);
        rendered_fields.push('\n');
    }

    let updated = if parsed.frontmatter.is_some() && normalized.starts_with("---\n") {
        let fm_end = find_frontmatter_close(&normalized)
            .ok_or_else(|| "could not locate closing frontmatter delimiter".to_owned())?;
        let fm_body = &normalized[4..fm_end.content_end];
        let cleaned_fm = strip_yaml_keys(fm_body, &keys_to_replace);
        let tail = &normalized[fm_end.after_close..];
        let mut out = String::with_capacity(normalized.len() + rendered_fields.len() + 16);
        out.push_str("---\n");
        out.push_str(&cleaned_fm);
        if !cleaned_fm.is_empty() && !cleaned_fm.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&rendered_fields);
        out.push_str("---");
        out.push_str(tail);
        out
    } else {
        let mut out = String::with_capacity(normalized.len() + rendered_fields.len() + 16);
        out.push_str("---\n");
        out.push_str(&rendered_fields);
        out.push_str("---\n\n");
        out.push_str(&normalized);
        out
    };

    // Fail-closed round-trip parse check and body hash invariance assertion.
    let reparsed = parse_document(updated.as_bytes())
        .map_err(|_| "rewritten note failed UTF-8 parse".to_owned())?;
    if let Some(err) = reparsed.frontmatter_error {
        return Err(format!("rewritten note has invalid frontmatter: {err}"));
    }
    let after_body_hash = body_content_sha256(updated.get(reparsed.body_offset..).unwrap_or(""));
    if before_body_hash != after_body_hash {
        return Err("body hash invariance violated while applying enrichment".to_owned());
    }

    Ok(updated)
}

struct FmClose {
    content_end: usize,
    after_close: usize,
}

fn find_frontmatter_close(text: &str) -> Option<FmClose> {
    let mut offset = 4usize; // right after opening "---\n"
    for line in text[4..].split_inclusive('\n') {
        let trimmed = line.trim_end_matches('\n');
        // Match `tb_syntax::parse_frontmatter` strictly: only `---` closes frontmatter.
        if trimmed == "---" {
            return Some(FmClose {
                content_end: offset,
                after_close: offset + trimmed.len(),
            });
        }
        offset += line.len();
    }
    None
}

fn strip_yaml_keys(fm_body: &str, keys: &[&str]) -> String {
    let mut out = String::with_capacity(fm_body.len());
    let mut skipping_block = false;
    let mut flow_depth: i32 = 0;

    let count_flow_delta = |s: &str| -> i32 {
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;
        let mut delta = 0i32;
        for ch in s.chars() {
            if escaped {
                escaped = false;
                continue;
            }
            if in_double {
                if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_double = false;
                }
                continue;
            }
            if in_single {
                if ch == '\'' {
                    in_single = false;
                }
                continue;
            }
            match ch {
                '"' => in_double = true,
                '\'' => in_single = true,
                '#' => break,
                '[' | '{' => delta += 1,
                ']' | '}' => delta -= 1,
                _ => {}
            }
        }
        delta
    };

    for line in fm_body.split_inclusive('\n') {
        let is_indented = line.starts_with(' ') || line.starts_with('\t');
        let trimmed = line.trim();
        if skipping_block {
            if flow_depth > 0 {
                flow_depth = (flow_depth + count_flow_delta(line)).max(0);
                if flow_depth == 0 {
                    skipping_block = false;
                }
                continue;
            }
            if is_indented
                || trimmed.is_empty()
                || trimmed == "-"
                || trimmed.starts_with("- ")
                || trimmed.starts_with("-\t")
            {
                continue;
            }
            skipping_block = false;
        }
        if !is_indented && let Some((k, rest)) = line.split_once(':') {
            let key_clean = k.trim().trim_matches('"').trim_matches('\'');
            if keys.contains(&key_clean) {
                skipping_block = true;
                flow_depth = count_flow_delta(rest).max(0);
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

/// Summary and atomic write operations produced by [`build_enrichment_apply_plan`].
#[derive(Clone, Debug)]
pub struct EnrichmentApplyPlan {
    pub ops_by_repo: BTreeMap<String, Vec<IngestWriteOp>>,
    pub planned_writes: usize,
    pub skipped_unchanged: usize,
    pub skipped_already_enriched: usize,
    pub skipped_missing_cache: usize,
    pub errors: Vec<String>,
}

/// Build CAS-bound write operations for every note matching a valid cache entry for
/// `(note_hash, model, prompt_version)`.
///
/// Refuses to mutate read-only `dependency` workspace members (`CODING_STANDARDS.md` Rule 17).
#[must_use]
pub fn build_enrichment_apply_plan(
    snapshot: &VaultSnapshot,
    cache: &EnrichmentCache,
    model: &str,
    prompt_version: &str,
    repo_filter: Option<&str>,
    overwrite_existing: bool,
) -> EnrichmentApplyPlan {
    let mut ops_by_repo: BTreeMap<String, Vec<IngestWriteOp>> = BTreeMap::new();
    let mut planned_writes = 0usize;
    let mut skipped_unchanged = 0usize;
    let mut skipped_already_enriched = 0usize;
    let mut skipped_missing_cache = 0usize;
    let mut errors = Vec::new();

    if let Some(target_repo) = repo_filter
        && let Some(meta) = snapshot.members.get(target_repo)
        && meta.role.eq_ignore_ascii_case("dependency")
    {
        errors.push(format!(
            "policy/read_only_member: refusing to enrich read-only dependency repo '{target_repo}'"
        ));
        return EnrichmentApplyPlan {
            ops_by_repo,
            planned_writes,
            skipped_unchanged,
            skipped_already_enriched,
            skipped_missing_cache,
            errors,
        };
    }

    for (loc, file) in &snapshot.files {
        if repo_filter.is_some_and(|r| loc.repo != r) {
            continue;
        }
        if snapshot
            .members
            .get(&loc.repo)
            .is_some_and(|m| m.role.eq_ignore_ascii_case("dependency"))
        {
            continue;
        }
        let body = file.body();
        if body.trim().is_empty() {
            continue;
        }
        if !overwrite_existing && has_enrichment_fields(file.doc.frontmatter.as_ref()) {
            skipped_already_enriched += 1;
            continue;
        }
        let note_hash = body_content_sha256(body);
        let Some(rec) = cache.get(&note_hash, model, prompt_version) else {
            skipped_missing_cache += 1;
            continue;
        };
        if rec.keywords.is_empty() && rec.search_questions.is_empty() {
            skipped_unchanged += 1;
            continue;
        }
        match apply_enrichment_to_note(&file.raw_text, &rec.keywords, &rec.search_questions) {
            Ok(updated) => {
                if updated == file.raw_text {
                    skipped_unchanged += 1;
                } else {
                    ops_by_repo
                        .entry(loc.repo.clone())
                        .or_default()
                        .push(IngestWriteOp {
                            locator: loc.clone(),
                            content: updated,
                            expected_hash: Some(file.short_hash.clone()),
                        });
                    planned_writes += 1;
                }
            }
            Err(err) => errors.push(format!("{}/{}: {err}", loc.repo, loc.path)),
        }
    }

    EnrichmentApplyPlan {
        ops_by_repo,
        planned_writes,
        skipped_unchanged,
        skipped_already_enriched,
        skipped_missing_cache,
        errors,
    }
}

/// Build an in-memory enriched [`SearchIndex`] overlaying cached `(keywords, search_questions)`
/// onto a [`VaultSnapshot`] WITHOUT modifying disk — used for paired G5 pre-apply evaluation.
#[must_use]
pub fn build_simulated_enriched_index(
    snapshot: &VaultSnapshot,
    cache: &EnrichmentCache,
    model: &str,
    prompt_version: &str,
) -> (SearchIndex, usize) {
    let mut enriched_count = 0usize;
    let mut docs_owned: Vec<(Locator, String, String, String, FreshnessMeta)> =
        Vec::with_capacity(snapshot.files.len());

    for (loc, file) in &snapshot.files {
        let body = file.body().to_owned();
        let desc = file.description.clone().unwrap_or_default();
        let note_hash = body_content_sha256(&body);
        let mut enrich_parts = Vec::new();
        if let Some(fm) = file.doc.frontmatter.as_ref() {
            for key in ["keywords", "search_questions", "questions", "aliases"] {
                match fm.get(key) {
                    Some(FrontmatterValue::String(s)) if !s.trim().is_empty() => {
                        enrich_parts.push(s.trim().to_owned());
                    }
                    Some(FrontmatterValue::Seq(items)) => {
                        for it in items {
                            if let FrontmatterValue::String(s) = it
                                && !s.trim().is_empty()
                            {
                                enrich_parts.push(s.trim().to_owned());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(rec) = cache.get(&note_hash, model, prompt_version)
            && (!rec.keywords.is_empty() || !rec.search_questions.is_empty())
        {
            enriched_count += 1;
            enrich_parts.extend(rec.keywords.iter().cloned());
            enrich_parts.extend(rec.search_questions.iter().cloned());
        }
        let fm = file.doc.frontmatter.as_ref();
        let get_str = |keys: &[&str]| -> Option<&str> {
            let fm = fm?;
            for k in keys {
                if let Some(FrontmatterValue::String(s)) = fm.get(*k)
                    && !s.trim().is_empty()
                {
                    return Some(s.trim());
                }
            }
            None
        };
        let freshness = FreshnessMeta::from_fields(
            get_str(&["status"]),
            get_str(&["freshness"]),
            file.declared_type.as_deref().or_else(|| get_str(&["type"])),
            &loc.path,
            get_str(&["updated", "modified", "captured", "created", "date"]),
        );
        docs_owned.push((loc.clone(), body, desc, enrich_parts.join(" "), freshness));
    }

    let index = SearchIndex::build_rich(
        docs_owned
            .iter()
            .map(|(loc, body, desc, enr, fresh)| SearchDocInput {
                locator: loc.clone(),
                body: body.as_str(),
                description: desc.as_str(),
                enrichment: enr.as_str(),
                freshness: *fresh,
            }),
        None,
    );
    (index, enriched_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_hash_is_invariant_across_enrichment_application_and_roundtrip_parses() {
        let orig = "---\ntype: pattern\ndescription: \"Retry loop\"\n---\n\n# Retry Loop\n\nUse exponential backoff with jitter.\n";
        let h_before =
            body_content_sha256("# Retry Loop\n\nUse exponential backoff with jitter.\n");

        let updated = apply_enrichment_to_note(
            orig,
            &["backoff".into(), "jitter".into(), "Backoff".into()],
            &["How to configure retry backoff?".into()],
        )
        .expect("apply enrichment");

        assert!(updated.contains("type: pattern"));
        assert!(updated.contains("description: \"Retry loop\""));
        assert!(updated.contains("keywords: [\"backoff\",\"jitter\"]"));
        assert!(updated.contains("search_questions: [\"How to configure retry backoff?\"]"));

        let parsed = parse_document(updated.as_bytes()).expect("utf8");
        assert!(parsed.frontmatter_error.is_none());
        let h_after = body_content_sha256(&updated[parsed.body_offset..]);
        assert_eq!(h_before, h_after);

        // Second application with different values cleanly replaces prior keys without duplication.
        let updated2 = apply_enrichment_to_note(
            &updated,
            &["retry".into()],
            &["Why add jitter to retries?".into()],
        )
        .expect("reapply");
        assert_eq!(updated2.matches("keywords:").count(), 1);
        assert_eq!(updated2.matches("search_questions:").count(), 1);
        assert!(updated2.contains("keywords: [\"retry\"]"));
    }

    #[test]
    fn cache_jsonl_fails_closed_on_corrupt_line_and_keys_by_hash_model_prompt() {
        let hash = body_content_sha256("Hello world");
        let rec = EnrichmentRecord {
            schema_version: ENRICH_CACHE_SCHEMA_VERSION.to_owned(),
            repo: "vault".to_owned(),
            path: "a.md".to_owned(),
            note_hash: hash.clone(),
            model: "gpt-4o-mini".to_owned(),
            prompt_version: "v1".to_owned(),
            keywords: vec!["hello".into()],
            search_questions: vec!["What says hello?".into()],
        };
        let mut cache = EnrichmentCache::new();
        cache.insert(rec);
        let serialized = cache.to_jsonl();
        let reloaded = EnrichmentCache::from_jsonl(&serialized).expect("reload valid cache");
        assert!(reloaded.get(&hash, "gpt-4o-mini", "v1").is_some());
        assert!(reloaded.get(&hash, "gpt-4o-mini", "v2").is_none());
        assert!(reloaded.get(&hash, "other-model", "v1").is_none());

        let corrupt = format!("{serialized}{{broken json\n");
        let err = EnrichmentCache::from_jsonl(&corrupt).expect_err("must fail closed");
        assert!(err.contains("line 2"));
    }

    #[test]
    fn heuristic_enrich_produces_deterministic_keywords_and_questions() {
        let (kw, qs) = heuristic_enrich(
            "Zero-TOCTOU Dirfd Confinement",
            Some("How tb-fs-git resolves workspace paths using openat O_NOFOLLOW"),
            "# Zero-TOCTOU Dirfd Confinement\n\n## Path Traversal Guard\nAlways use openat with O_NOFOLLOW.\n",
        );
        assert!(!kw.is_empty());
        assert!(kw.iter().any(|k| k == "openat" || k == "confinement"));
        assert!(!qs.is_empty());
        assert!(qs.iter().any(|q| q.contains("Zero-TOCTOU")));
    }

    #[test]
    fn partial_update_multiline_flow_arrays_quoted_keys_and_dependency_guard_work() {
        // 1. Multiline flow array + quoted key + partial update preserves existing `search_questions`.
        let tricky = "---\ntype: pattern\n\"keywords\": [\n  \"old1\",\n  \"old2\"\n  ]\nsearch_questions:\n  - \"Keep this existing question?\"\n---\n\n# Body\n\nContent.\n";
        let kw_only = apply_enrichment_to_note(tricky, &["fresh".into()], &[])
            .expect("keywords-only update must preserve search_questions");
        assert!(kw_only.contains("keywords: [\"fresh\"]"));
        assert!(kw_only.contains("Keep this existing question?"));
        assert!(!kw_only.contains("old1"));
        assert!(!kw_only.contains("\"keywords\":"));
        let reparsed = parse_document(kw_only.as_bytes()).expect("valid utf8");
        assert!(reparsed.frontmatter_error.is_none());

        // 2. Read-only `dependency` repo is refused by `build_enrichment_apply_plan`.
        let mut snap = VaultSnapshot::empty();
        snap.members.insert(
            "dep".to_owned(),
            crate::MemberMeta {
                repo: "dep".to_owned(),
                root: "/tmp/dep".to_owned(),
                role: "dependency".to_owned(),
                deps: Vec::new(),
            },
        );
        let plan = build_enrichment_apply_plan(
            &snap,
            &EnrichmentCache::new(),
            "heuristic-v1",
            "v1",
            Some("dep"),
            false,
        );
        assert!(!plan.errors.is_empty());
        assert!(plan.errors[0].contains("dependency"));
    }
}
