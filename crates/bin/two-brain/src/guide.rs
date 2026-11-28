//! Offline agent guide (`2brain guide [topic]`) embedded at compile time from the single-source
//! docs (`skill/2brain/SKILL.md` and `docs/llms.txt`) so agent guidance never drifts from the binary.

const SKILL_OVERVIEW: &str = include_str!("../../../../skill/2brain/SKILL.md");
const LLMS_TXT: &str = include_str!("../../../../docs/llms.txt");

const TOPIC_SEARCH: &str = r#"# 2brain Guide: Search & Retrieval (SEM-09)

- Use `tb_search` for ranked multi-term and conceptual recall:
  4-field sparse-safe BM25 (`body 1.0`, `description 3.0`, `keywords 2.2`, `search_questions 1.4`)
  with type-aware freshness decay (durable types never decay; transient notes use half-life).
- Use `tb_grep` for exact literal substrings, identifiers, file names, or error codes.
- Override BM25 weights and freshness half-life in `workspace.yaml` (priority 1) or `.2brain/repo.yaml`
  under `search.weights` and `search.freshness`.
- Pass `max_bytes` or `max_tokens` to MCP tools to cap response size with graceful degradation.
"#;

const TOPIC_MUTATE: &str = r#"# 2brain Guide: Fail-Closed Mutations (SEM-02)

1. Read the target note completely with `tb_content` (`truncated == false`) to obtain `hash` (`sha256`) and `read_receipt`.
2. Pass `expected_hash` and `read_receipt` to `tb_edit_file`, `tb_write_file`, `tb_delete_file`, or `tb_rename`.
3. Mutations are refused by default unless the MCP server was launched with `--allow-mutations`.
4. Read-only `dependency` members in `workspace.yaml` are always refused for writes and ingest.
5. After editing, run `2brain check <workspace> --audit-descriptions --json` (done-when: `schema == 0`, `utf8_errors == 0`).
"#;

const TOPIC_ENRICH: &str = r#"# 2brain Guide: Offline Note Enrichment & G5 Gates

1. Plan missing cache entries:
   `2brain enrich plan <ws> --cache <cache.jsonl> --model <m> --prompt-version v1 --only-missing --json`
2. Populate cache outside the serving path (deterministic heuristic or LLM):
   `python3 scripts/enrich_llm.py <ws> --mode llm --concurrency 4 --suite <holdout.json> --cache <cache.jsonl>`
3. Verify paired G5 gate (`promote_candidate` required, 0 SEM-48 critical violations):
   `2brain enrich evaluate <ws> --cache <cache.jsonl> --model <m> --prompt-version v1 --suite <holdout.json> --json`
4. Apply atomically (dry-run first, then real apply):
   `2brain enrich apply <ws> --cache <cache.jsonl> --model <m> --prompt-version v1 --dry-run --json`
"#;

const TOPIC_INGEST: &str = r#"# 2brain Guide: Ingest & Recurring Error-Fix Atoms

- Supported formats: `auto`, `md`, `txt`, `html`, `csv`, `json`, `jsonl`, `error-fix`.
- Always preview with `--dry-run --json` before committing:
  `2brain ingest <ws> <source> --format error-fix --type error-fix --dest inbox/errors --dry-run --json`
- Extract recurring errors from logs/JSONL declaratively:
  `python3 scripts/extract_error_fixes.py --input <app.log> --min-occurrences 2 --workspace <ws>`
"#;

const TOPIC_WORKFLOWS: &str = r#"# 2brain Guide: Durable Workflows (D08/D09/D10) & Wakeup

- Orient in ~500 tokens at session start:
  `2brain wakeup <ws> --max-tokens 500`
- Persist cross-client task handoffs with CAS revisions:
  `2brain task checkpoint` / `2brain task show` / `2brain task list` (MCP: `tb_task_checkpoint` / `tb_task_resume` / `tb_task_list`)
- Run declarative procedures with independent postconditions and receipts:
  `2brain procedure run <ws> <spec.json>`
"#;

const TOPIC_EVAL: &str = r#"# 2brain Guide: G5 Paired Evaluation & Semantic Suite

- Paired arm evaluation (fail-closed on duplicate tasks, non-finite values, or regressions):
  `2brain experiment evaluate-arm <input.json> --json`
- Diagnostic bounded Personalized PageRank (origin-deduplicated, CV4 convergence trace):
  `2brain experiment ppr <ws> --seed <repo::path> --json`
- Black-box semantic acceptance harness (21 cases + SEM-48 control):
  `python3 tools/semantic/run_semantic_suite.py --bin <path/to/2brain>`
"#;

pub const AVAILABLE_TOPICS: &[&str] = &[
    "overview",
    "llms",
    "search",
    "mutate",
    "enrich",
    "ingest",
    "workflows",
    "eval",
];

/// Return the embedded guide markdown for `topic` (defaults to `"overview"`).
pub fn render_guide(topic: Option<&str>) -> Result<(&'static str, &'static str), String> {
    let key = topic
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("overview")
        .to_ascii_lowercase();
    match key.as_str() {
        "overview" | "skill" => Ok(("overview", SKILL_OVERVIEW)),
        "llms" | "llms.txt" | "all" => Ok(("llms", LLMS_TXT)),
        "search" | "retrieval" => Ok(("search", TOPIC_SEARCH)),
        "mutate" | "mutations" | "write" => Ok(("mutate", TOPIC_MUTATE)),
        "enrich" | "enrichment" => Ok(("enrich", TOPIC_ENRICH)),
        "ingest" | "error-fix" => Ok(("ingest", TOPIC_INGEST)),
        "workflows" | "workflow" | "wakeup" => Ok(("workflows", TOPIC_WORKFLOWS)),
        "eval" | "experiment" => Ok(("eval", TOPIC_EVAL)),
        other => Err(format!(
            "unknown guide topic '{other}' (available: {})",
            AVAILABLE_TOPICS.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_guide_topics_render_non_empty_and_unknown_fails_closed() {
        for &t in AVAILABLE_TOPICS {
            let (resolved, body) = render_guide(Some(t)).expect("known topic must render");
            assert!(!resolved.is_empty());
            assert!(body.len() > 100, "topic {t} guide unexpectedly short");
        }
        assert!(render_guide(None).is_ok());
        assert!(render_guide(Some("nonexistent-topic")).is_err());
    }
}
