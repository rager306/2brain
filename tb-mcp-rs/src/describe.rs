//! How the tool catalogue presents itself: the one-line cut every default surface
//! carries, and the task-word ranking `tb_search_tools` runs over the same catalogue.
//!
//! The concise profile carries the first sentence of each description and withholds the rest until
//! `tb_describe_tool` asks for it, so this cut is what every agent reads by default. It is its own module
//! because the ratchet on `lib.rs` counts a line whether or not it is about routing: the rule there is to
//! move a responsibility out rather than raise the ceiling.

use crate::capability::Capability;
use rmcp::model::Tool;

/// Rank tools for a query: a term in the name weighs most, then a parameter
/// name, then the description. Deterministic, and cheap enough to run per call.
pub(crate) fn search_tools(
    tools: &[Tool],
    query: &str,
    limit: usize,
    capability: Option<Capability>,
) -> Vec<Tool> {
    let terms: Vec<String> = tokenize(query);
    if terms.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u32, &Tool)> = tools
        .iter()
        .filter(|tool| match capability {
            Some(wanted) => crate::capability::of(tool.name.as_ref()) == Some(wanted),
            None => true,
        })
        .filter_map(|tool| {
            let name = tokenize(tool.name.as_ref());
            let text = tokenize(tool.description.as_deref().unwrap_or(""));
            let params: Vec<String> = tool
                .input_schema
                .get("properties")
                .and_then(serde_json::Value::as_object)
                .map(|properties| properties.keys().flat_map(|key| tokenize(key)).collect())
                .unwrap_or_default();
            let mut score = 0;
            for term in &terms {
                if name.contains(term) {
                    score += 3;
                }
                if params.contains(term) {
                    score += 2;
                }
                if text.iter().any(|word| word == term) {
                    score += 1;
                }
                if text.iter().any(|word| word.starts_with(term.as_str())) {
                    score += 1;
                }
            }
            (score > 0).then_some((score, tool))
        })
        .collect();
    scored.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.name.cmp(&right.name))
    });
    scored
        .into_iter()
        .take(limit)
        .map(|(_, tool)| tool.clone())
        .collect()
}

/// Lowercase word tokens; underscores and dots split so `tb_content` matches
/// "content" and `io.arsumbris` matches "2brain".
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The first sentence, on a clause boundary, never wider than the limit.
///
/// Cuts in this order: a sentence end under the limit, then a separator (`;`, `:`, ` — `, `,`), then the
/// last word boundary, and only then a raw character count. Cutting at a flat 140 split eight descriptions
/// mid-phrase the day concise became the default — `tb_follow` lost the inside of a backtick enumeration —
/// and a description that stops mid-sentence reads as a truncated field rather than a short one.
pub(crate) fn first_sentence(description: Option<&str>) -> String {
    const LIMIT: usize = 140;
    const CLAUSES: [&str; 4] = ["; ", ": ", " — ", ", "];

    let text = description.unwrap_or("").trim();
    if text.chars().count() <= LIMIT {
        return text.to_owned();
    }
    let head: String = text.chars().take(LIMIT).collect();
    if let Some(cut) = head.find(". ").filter(|cut| *cut + 2 < LIMIT) {
        return text[..=cut].trim().to_owned();
    }
    for separator in CLAUSES {
        if let Some(cut) = head.rfind(separator) {
            return text[..cut].trim_end().to_owned();
        }
    }
    match head.rfind(' ') {
        Some(cut) if cut > LIMIT / 3 => text[..cut].trim_end().to_owned(),
        _ => head,
    }
}

#[cfg(test)]
mod tests {
    use super::first_sentence;

    /// A short description is returned whole, with its edges trimmed.
    #[test]
    fn a_short_description_is_untouched() {
        assert_eq!(first_sentence(Some("  Rank notes.  ")), "Rank notes.");
        assert_eq!(first_sentence(None), "");
    }

    /// A sentence end under the limit wins over every separator, so a description with two sentences keeps one.
    #[test]
    fn a_sentence_end_is_preferred() {
        // Longer than the limit overall, so the cut has to choose; the first sentence ends well under it.
        let text = "Rank notes by relevance. BM25 over each note's body and its frontmatter description, with the \
                    description weighted higher, a floor on matching terms, a rare-term escape, and a flag that \
                    says a page is only common words.";
        assert!(text.chars().count() > 140);
        assert_eq!(first_sentence(Some(text)), "Rank notes by relevance.");
    }

    /// With no sentence end under the limit the cut lands on a clause, never inside a word or a run of
    /// backticked names — the shape that broke `tb_follow` when the cut was a flat character count.
    #[test]
    fn a_long_clause_cuts_on_a_boundary() {
        let text = "Follow a reference: `a`, `b`, `c`, `d`, `e`, `f`, `g`, `h`, `i`, `j`, `k`, `l`, \
                    `m`, `n`, `o`, `p`, `q`, `r`, `s`, `t`, `u`, `v`, `w`, `x`, `y`, `z` and more.";
        let cut = first_sentence(Some(text));
        assert!(cut.chars().count() <= 140, "{cut}");
        assert!(
            !cut.ends_with(','),
            "a trailing separator reads as truncation: {cut}"
        );
        assert!(!cut.ends_with(' '), "{cut}");
        assert!(text.starts_with(cut.as_str()), "{cut}");
    }

    /// A single unbroken run longer than the limit is cut by characters — there is no boundary to find.
    #[test]
    fn an_unbroken_run_falls_back_to_the_character_limit() {
        let text = "x".repeat(200);
        assert_eq!(first_sentence(Some(&text)).chars().count(), 140);
    }
}
