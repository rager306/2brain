//! Pure-domain zero-LLM query classifier, continuous `alpha` router, and `KeywordSoup` splitter
//! (0 I/O, Miri-clean; clean-room synthesis of Memorilla complement routing and continuous query
//! conditioning for 2brain).
//!
//! Routes literal/exact queries toward 4-field BM25 (`bm25` boost, `ppr`/`semantic` dampened)
//! and conceptual/multi-entity queries toward graph propagation (`ppr` boost) and semantic recall.

use serde::{Deserialize, Serialize};

/// Coarse query intent class in strict precedence order.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryClass {
    /// Multi-disjunct boolean/comma keyword soup (split into OR branches, skip expansion).
    KeywordSoup,
    /// Repo-qualified locator, file path, or block-id anchor (`vault::a/b.md`, `ops/foo.md`, `^blk`).
    Locator,
    /// Exact identifier / code symbol / error code (`E0308`, `SearchWeights`, `tb_edit_file`).
    Symbol,
    /// Natural-language conceptual or multi-entity synthesis question.
    Concept,
}

/// Multipliers applied to retrieval channels based on [`QueryClass`], continuous `alpha`, and
/// optional prose mode (Markdown knowledge vault profile).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueryChannelProfile {
    pub class: QueryClass,
    /// Continuous interpolation parameter in `[0.15, 0.50]` (0.15 = literal locator, 0.50 = NL concept).
    pub alpha: f64,
    pub bm25_mult: f64,
    pub semantic_mult: f64,
    pub ppr_mult: f64,
    pub prose_mode: bool,
}

const COMMON_STOPWORDS: &[&str] = &[
    "the",
    "and",
    "for",
    "with",
    "that",
    "this",
    "from",
    "into",
    "about",
    "how",
    "why",
    "what",
    "when",
    "where",
    "which",
    "who",
    "does",
    "not",
    "without",
    "can",
    "are",
    "was",
    "were",
    "use",
    "как",
    "что",
    "где",
    "когда",
    "почему",
    "для",
    "при",
    "без",
    "или",
    "это",
    "все",
    "над",
    "под",
    "через",
    "чтобы",
    "если",
    "его",
    "она",
    "они",
];

fn is_stopword(token: &str) -> bool {
    let cleaned = token
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    !cleaned.is_empty() && COMMON_STOPWORDS.contains(&cleaned.as_str())
}

fn looks_like_identifier(token: &str) -> bool {
    let t = token.trim_matches(|c: char| {
        matches!(
            c,
            '"' | '\'' | ',' | ';' | '(' | ')' | '[' | ']' | '.' | '?' | '!' | ':'
        )
    });
    if t.len() < 2 {
        return false;
    }
    if t.contains("::") || t.contains('_') || t.starts_with('^') {
        return true;
    }
    // Dotted symbol like `tb.engine` (must have alphanumeric on BOTH sides of `.`).
    if let Some((l, r)) = t.split_once('.')
        && !l.is_empty()
        && !r.is_empty()
        && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && r.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return true;
    }
    // Error code or uppercase+digit code (e.g., E0308, SEM-48, BM25)
    let has_upper = t.chars().any(|c| c.is_ascii_uppercase());
    let has_lower = t.chars().any(|c| c.is_ascii_lowercase());
    let has_digit = t.chars().any(|c| c.is_ascii_digit());
    if has_upper && has_digit {
        return true;
    }
    // CamelCase transition (e.g., SearchWeights, TypedLinkGraph)
    if has_upper && has_lower {
        let mut prev_lower = false;
        for ch in t.chars() {
            if prev_lower && ch.is_ascii_uppercase() {
                return true;
            }
            prev_lower = ch.is_ascii_lowercase();
        }
    }
    // All-uppercase acronym of length >= 2 (e.g., CSR, PPR, MCP, FTS5)
    t.len() >= 2
        && t.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
        && t.chars().any(|c| c.is_ascii_uppercase())
}

fn looks_like_locator(single_token: &str) -> bool {
    let t = single_token.trim_matches(|c: char| matches!(c, '[' | ']' | '"' | '\''));
    if t.starts_with('^') && t.len() >= 2 {
        return true;
    }
    // Repo-qualified note locator (`vault::ops/runbook.md` or `vault::note.md`): right side has `/` or file extension.
    if let Some((repo, rest)) = t.split_once("::") {
        let valid_repo = !repo.is_empty()
            && repo
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'));
        let looks_like_path =
            rest.contains('/') || rest.ends_with(".md") || rest.ends_with(".yaml");
        if valid_repo && looks_like_path {
            return true;
        }
    }
    // Explicit markdown/yaml file path, or multi-segment lowercase/slug path (`ops/runbook.md`, `f/patterns/a`).
    if (t.ends_with(".md") || t.ends_with(".yaml")) && t.chars().any(|c| c.is_alphanumeric()) {
        return true;
    }
    if t.contains('/') {
        let segs: Vec<&str> = t.split('/').collect();
        // Exclude short prose/date slashes like `CI/CD`, `read/write`, `2026/11/28`.
        let all_digits = segs.iter().all(|s| s.chars().all(|c| c.is_ascii_digit()));
        if all_digits {
            return false;
        }
        if segs.len() >= 3
            || (segs.len() == 2
                && (segs[1].contains('.') || segs[1].contains('-') || segs[1].contains('_')))
        {
            return segs.iter().all(|s| !s.is_empty());
        }
    }
    false
}

/// Classify a raw search query into [`QueryClass`] without any LLM or I/O.
#[must_use]
pub fn classify_query(query: &str) -> QueryClass {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return QueryClass::Concept;
    }

    // Comma-separated compact keyword list without spaces (`alpha,beta,gamma`).
    if trimmed.contains(',') {
        let comma_frags: Vec<&str> = trimmed
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        if comma_frags.len() >= 3
            && comma_frags
                .iter()
                .all(|f| f.split_whitespace().count() <= 3)
            && !comma_frags.iter().any(|f| is_stopword(f))
        {
            return QueryClass::KeywordSoup;
        }
    }

    let raw_tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if raw_tokens.len() == 1 {
        let tok = raw_tokens[0];
        if looks_like_locator(tok) {
            return QueryClass::Locator;
        }
        if looks_like_identifier(tok) {
            return QueryClass::Symbol;
        }
        return QueryClass::Concept;
    }

    let bool_ops = raw_tokens
        .iter()
        .filter(|&&t| matches!(t, "OR" | "AND" | "|" | "||"))
        .count();
    let quoted_frags = trimmed.matches('"').count() / 2;

    let non_op_tokens: Vec<&str> = raw_tokens
        .iter()
        .copied()
        .filter(|&t| !matches!(t, "OR" | "AND" | "|" | "||"))
        .collect();
    let total = non_op_tokens.len().max(1) as f64;
    let stop_ratio = non_op_tokens.iter().filter(|&&t| is_stopword(t)).count() as f64 / total;
    let ident_ratio = non_op_tokens
        .iter()
        .filter(|&&t| looks_like_identifier(t))
        .count() as f64
        / total;

    if bool_ops >= 2
        || quoted_frags >= 3
        || (raw_tokens.len() >= 8 && bool_ops >= 1 && stop_ratio < 0.15 && ident_ratio > 0.6)
        || (non_op_tokens.len() >= 6 && stop_ratio <= 0.15 && ident_ratio >= 0.8)
    {
        return QueryClass::KeywordSoup;
    }

    QueryClass::Concept
}

/// Compute continuous interpolation parameter `alpha` in `[0.15, 0.50]` and channel multipliers
/// for a query.
#[must_use]
pub fn query_channel_profile(query: &str, prose_mode: bool) -> QueryChannelProfile {
    let class = classify_query(query);
    let alpha = match class {
        QueryClass::Locator => 0.15,
        QueryClass::Symbol | QueryClass::KeywordSoup => 0.30,
        QueryClass::Concept => {
            let tokens: Vec<&str> = query.split_whitespace().collect();
            let n = tokens.len();
            if n == 0 {
                0.50
            } else {
                let nf = n as f64;
                let id_frac =
                    tokens.iter().filter(|&&t| looks_like_identifier(t)).count() as f64 / nf;
                let stop_frac = tokens.iter().filter(|&&t| is_stopword(t)).count() as f64 / nf;
                let len_penalty = if n > 2 { 0.12 * ((n - 2) as f64) } else { 0.0 };
                let id_score = (id_frac - stop_frac - len_penalty).clamp(0.0, 1.0);
                (0.50 + id_score * (0.30 - 0.50)).clamp(0.15, 0.50)
            }
        }
    };

    let t = ((alpha - 0.15) / (0.50 - 0.15)).clamp(0.0, 1.0);
    let mut bm25_mult = 1.25 + t * (1.00 - 1.25);
    let mut semantic_mult = 0.45 + t * (1.00 - 0.45);
    let ppr_mult = 0.40 + t * (1.30 - 0.40);

    if prose_mode {
        bm25_mult *= 1.25;
        semantic_mult *= 1.30;
    }

    QueryChannelProfile {
        class,
        alpha,
        bm25_mult,
        semantic_mult,
        ppr_mult,
        prose_mode,
    }
}

/// Split a [`QueryClass::KeywordSoup`] query into clean individual disjunct fragments
/// aligned with all `KeywordSoup` shapes (`OR`/`AND`/`|`/`||`, commas, quoted phrases, or bare
/// identifier lists).
#[must_use]
pub fn split_keyword_soup(query: &str) -> Vec<String> {
    fn push_unique(out: &mut Vec<String>, raw: &str) {
        let cleaned = raw
            .trim()
            .trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | ','))
            .trim();
        if !cleaned.is_empty() && !out.iter().any(|existing: &String| existing == cleaned) {
            out.push(cleaned.to_owned());
        }
    }

    let mut out = Vec::new();

    // First handle explicit boolean/pipe/comma delimiters (normalizing whitespace around ops).
    let collapsed = query.split_whitespace().collect::<Vec<_>>().join(" ");
    let has_explicit_sep = collapsed.contains(" OR ")
        || collapsed.contains(" AND ")
        || collapsed.contains('|')
        || collapsed.contains(',');

    if has_explicit_sep {
        let normalized = collapsed
            .replace("||", " | ")
            .replace(" OR ", " | ")
            .replace(" AND ", " | ");
        for part in normalized.split('|') {
            for sub in part.split(',') {
                push_unique(&mut out, sub);
            }
        }
        return out;
    }

    // Handle quoted phrase soup (`"phrase a" "phrase b" "phrase c"`).
    if query.matches('"').count() >= 4 {
        let mut inside = false;
        let mut cur = String::new();
        for ch in query.chars() {
            if ch == '"' {
                if inside {
                    push_unique(&mut out, &cur);
                    cur.clear();
                }
                inside = !inside;
            } else if inside {
                cur.push(ch);
            }
        }
        if !out.is_empty() {
            return out;
        }
    }

    // Fallback: bare multi-identifier soup.
    for tok in query.split_whitespace() {
        push_unique(&mut out, tok);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_locator_symbol_soup_and_concept_accurately() {
        assert_eq!(
            classify_query("vault::f/patterns/a.md"),
            QueryClass::Locator
        );
        assert_eq!(classify_query("^blk-42"), QueryClass::Locator);
        assert_eq!(classify_query("SearchWeights"), QueryClass::Symbol);
        assert_eq!(classify_query("E0308"), QueryClass::Symbol);
        assert_eq!(classify_query("tb_edit_file"), QueryClass::Symbol);
        assert_eq!(classify_query("std::sync::Arc"), QueryClass::Symbol);
        assert_eq!(classify_query("CI/CD"), QueryClass::Concept);
        assert_eq!(classify_query("memory."), QueryClass::Concept);
        assert_eq!(classify_query("Cross-platform"), QueryClass::Concept);
        assert_eq!(
            classify_query("BM25 OR PPR OR MinHash"),
            QueryClass::KeywordSoup
        );
        assert_eq!(classify_query("alpha,beta,gamma"), QueryClass::KeywordSoup);
        assert_eq!(
            classify_query("Как привязать память к символам кода?"),
            QueryClass::Concept
        );
    }

    #[test]
    fn continuous_profile_favors_bm25_on_locator_and_ppr_on_concept() {
        let loc = query_channel_profile("ops/runbook.md", false);
        let nl = query_channel_profile("Как работает графовая диффузия по заметкам?", false);
        assert!(loc.bm25_mult > nl.bm25_mult);
        assert!(nl.ppr_mult > loc.ppr_mult);
        assert!(nl.semantic_mult > loc.semantic_mult);

        let prose =
            query_channel_profile("how does graph diffusion connect multiple concepts", true);
        assert!(prose.bm25_mult > nl.bm25_mult);
        assert!(prose.semantic_mult > nl.semantic_mult);
    }

    #[test]
    fn splits_all_keyword_soup_shapes_cleanly() {
        assert_eq!(
            split_keyword_soup("\"BM25\"  OR  \"PPR\" | MinHash, BM25"),
            vec!["BM25", "PPR", "MinHash"]
        );
        assert_eq!(split_keyword_soup("a AND b AND c"), vec!["a", "b", "c"]);
        assert_eq!(
            split_keyword_soup("\"foo bar\" \"baz qux\" \"hello world\""),
            vec!["foo bar", "baz qux", "hello world"]
        );
    }
}
