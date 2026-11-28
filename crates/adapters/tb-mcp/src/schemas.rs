//! Complete JSON Schema catalog for `--tools full` and `tb_describe_tool`,
//! plus structured `reason_code` + `recovery_hint` error classification.

use std::sync::Arc;

use serde_json::{Map, Value, json};

/// Return the full JSON Schema object for any 2brain tool (54 core + 9 cognitive/decision).
#[must_use]
pub fn full_schema_for(name: &str) -> Arc<Map<String, Value>> {
    let schema = match name {
        "tb_overview" | "tb_graph_health" | "tb_imports" | "tb_top_level_dirs" | "tb_types"
        | "tb_type_tree" | "tb_typed" | "tb_instances" => json!({
            "type": "object",
            "properties": {
                "repo": { "type": "string", "description": "Optional workspace member repository slug" }
            }
        }),
        "tb_members" => json!({
            "type": "object",
            "properties": {}
        }),
        "tb_resolve_member" => json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "path": { "type": "string", "description": "Absolute or member-relative path to map to its workspace member" }
            }
        }),
        "tb_files" => json!({
            "type": "object",
            "properties": {
                "repo": { "type": "string" },
                "prefix": { "type": "string", "description": "Optional directory prefix filter" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "offset": { "type": "integer", "minimum": 0 }
            }
        }),
        "tb_content" => json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string", "description": "Member-relative POSIX path to the markdown note" },
                "offset_chars": { "type": "integer", "minimum": 0 },
                "max_chars": { "type": "integer", "minimum": 1, "description": "Omit to perform a complete write-arming read (SEM-02)" },
                "expected": {
                    "type": "object",
                    "properties": {
                        "sha256": { "type": "string", "description": "Conditional hydration digest; skips body when unchanged" }
                    }
                }
            }
        }),
        "tb_frontmatter" | "tb_anchors" | "tb_block_ids" | "tb_semantic_tokens"
        | "tb_references" | "tb_children" | "tb_resolved" => json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" }
            }
        }),
        "tb_follow" => json!({
            "type": "object",
            "required": ["target"],
            "properties": {
                "repo": { "type": "string" },
                "target": { "type": "string", "description": "Wikilink target stem or relative path" }
            }
        }),
        "tb_diagnostics" => json!({
            "type": "object",
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "offset": { "type": "integer", "minimum": 0 }
            }
        }),
        "tb_type" | "tb_subtypes" | "tb_instances_of" | "tb_candidates" => json!({
            "type": "object",
            "required": ["type"],
            "properties": {
                "repo": { "type": "string" },
                "type": { "type": "string", "description": "Type name (or qualified name::repo)" }
            }
        }),
        "tb_validate_value" => json!({
            "type": "object",
            "required": ["type", "field", "value"],
            "properties": {
                "repo": { "type": "string" },
                "type": { "type": "string" },
                "field": { "type": "string" },
                "value": { "description": "Candidate YAML/JSON field value to validate" }
            }
        }),
        "tb_search" => json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string", "description": "BM25 + proximity search query" },
                "repo": { "type": "string" },
                "type": { "type": "string", "description": "Optional type filter" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 100 }
            }
        }),
        "tb_grep" => json!({
            "type": "object",
            "required": ["pattern"],
            "properties": {
                "pattern": { "type": "string", "description": "Exact literal substring (SEM-09)" },
                "repo": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            }
        }),
        "tb_context" => json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string" },
                "repo": { "type": "string" },
                "max_notes": { "type": "integer", "minimum": 1, "maximum": 20 },
                "max_chars_per_note": { "type": "integer", "minimum": 100, "maximum": 20000 }
            }
        }),
        "tb_hubs" => json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            }
        }),
        "tb_neighborhood" => json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "depth": { "type": "integer", "minimum": 1, "maximum": 4 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            }
        }),
        "tb_describe_tool" => json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string", "description": "Exact tool name (e.g. tb_content)" }
            }
        }),
        "tb_search_tools" => json!({
            "type": "object",
            "required": ["query"],
            "properties": {
                "query": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 64 }
            }
        }),
        "tb_mutation_outcome" => json!({
            "type": "object",
            "required": ["request_id"],
            "properties": {
                "request_id": { "type": "string", "description": "32-hex idempotent request identifier" }
            }
        }),
        "tb_preview_mutation" => json!({
            "type": "object",
            "required": ["op", "path"],
            "properties": {
                "op": { "type": "string", "enum": ["create_file", "write_file", "edit_file", "delete_file", "rename"] },
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "content": { "type": "string" },
                "old_str": { "type": "string" },
                "new_str": { "type": "string" },
                "new_path": { "type": "string" }
            }
        }),
        "tb_reconcile" => json!({
            "type": "object",
            "required": ["path"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_rollback_preview" => json!({
            "type": "object",
            "properties": {
                "repo": { "type": "string" },
                "commit": { "type": "string", "description": "Git ref or SHA (default HEAD)" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 }
            }
        }),
        "tb_watch" => json!({
            "type": "object",
            "properties": {
                "since_generation": { "type": "integer", "minimum": 0 },
                "timeout_ms": { "type": "integer", "minimum": 10, "maximum": 30000 }
            }
        }),
        "tb_create_file" => json!({
            "type": "object",
            "required": ["repo", "path", "content"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "content": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_write_file" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "content"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string", "description": "Optional stateless MCP 2.0 HMAC receipt from whole tb_content" },
                "content": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_edit_file" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "old_str", "new_str"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "old_str": { "type": "string" },
                "new_str": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_delete_file" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_rename" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "new_path"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "new_path": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_assign_block_id" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "block_id"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "block_id": { "type": "string" },
                "line": { "type": "integer", "minimum": 1 },
                "match_text": { "type": "string" },
                "heading": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_rename_block_id" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "old_id", "new_id"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "old_id": { "type": "string" },
                "new_id": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_rename_type" => json!({
            "type": "object",
            "required": ["old_name", "new_name"],
            "properties": {
                "repo": { "type": "string" },
                "old_name": { "type": "string" },
                "new_name": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_inline" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "target"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "target": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_promote" => json!({
            "type": "object",
            "required": ["repo", "path", "expected_hash", "new_path"],
            "properties": {
                "repo": { "type": "string" },
                "path": { "type": "string" },
                "expected_hash": { "type": "string" },
                "read_receipt": { "type": "string" },
                "block_id": { "type": "string" },
                "heading": { "type": "string" },
                "new_path": { "type": "string" },
                "new_type": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_batch" => json!({
            "type": "object",
            "required": ["operations"],
            "properties": {
                "operations": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 64,
                    "items": { "type": "object" }
                },
                "request_id": { "type": "string" }
            }
        }),
        "tb_add_dep" | "tb_remove_dep" => json!({
            "type": "object",
            "required": ["dep"],
            "properties": {
                "repo": { "type": "string" },
                "dep": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_add_workspace_member" => json!({
            "type": "object",
            "required": ["name", "path"],
            "properties": {
                "name": { "type": "string" },
                "path": { "type": "string" },
                "role": { "type": "string", "enum": ["own", "dependency", "primary"] },
                "request_id": { "type": "string" }
            }
        }),
        "tb_remove_workspace_member" => json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": { "type": "string" },
                "request_id": { "type": "string" }
            }
        }),
        "tb_set_workspace_member_role" => json!({
            "type": "object",
            "required": ["name", "role"],
            "properties": {
                "name": { "type": "string" },
                "role": { "type": "string", "enum": ["own", "dependency", "primary"] },
                "request_id": { "type": "string" }
            }
        }),
        "tb_orient" => json!({
            "type": "object",
            "properties": {
                "suppressed_question_ids": { "type": "array", "items": { "type": "string" } },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_analyze_topology" => json!({
            "type": "object",
            "properties": {
                "peel_top_k": { "type": "integer", "minimum": 1, "maximum": 20 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 30 },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_detect_gaps" => json!({
            "type": "object",
            "properties": {
                "suppressed_question_ids": { "type": "array", "items": { "type": "string" } },
                "limit": { "type": "integer", "minimum": 1, "maximum": 30 },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_audit_bias" => json!({
            "type": "object",
            "required": ["response_text", "sources"],
            "properties": {
                "response_text": { "type": "string" },
                "sources": { "type": "array", "items": { "type": "object" } },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_audit_contradictions" => json!({
            "type": "object",
            "properties": {
                "as_of": { "type": "string", "description": "ISO-8601 date YYYY-MM-DD" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_fragility_report" => json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_diff_graphs" => json!({
            "type": "object",
            "required": ["from_ref"],
            "properties": {
                "from_ref": { "type": "string" },
                "to_ref": { "type": "string" },
                "repo": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
                "async_task": { "type": "boolean" }
            }
        }),
        "tb_record_decision" => json!({
            "type": "object",
            "required": ["target_id", "target_kind"],
            "properties": {
                "target_id": { "type": "string", "description": "QuestionId (q_...) or ConflictId (c_...)" },
                "id": { "type": "string", "description": "Alias for target_id" },
                "target_kind": { "type": "string", "enum": ["question", "conflict"] },
                "decision": { "type": "string", "enum": ["accept", "dismiss", "defer"], "description": "Optional when async_task=true with elicitation" },
                "rationale": { "type": "string" },
                "principal": { "type": "string" },
                "async_task": { "type": "boolean", "description": "If true and decision/rationale omitted, pauses in MCP SEP-2663 input_required elicitation" }
            }
        }),
        "tb_list_decisions" => json!({
            "type": "object",
            "properties": {
                "target_kind": { "type": "string", "enum": ["question", "conflict"] }
            }
        }),
        "tb_ingest" => json!({
            "type": "object",
            "required": ["content", "stem"],
            "properties": {
                "content": { "type": "string", "description": "Inline UTF-8 payload to convert and ingest (max 2 MiB)" },
                "stem": { "type": "string", "description": "Base filename stem or title used to derive target slug" },
                "format": { "type": "string", "enum": ["md", "txt", "html", "csv", "json", "jsonl"], "default": "md" },
                "dest": { "type": "string", "default": "inbox", "description": "Destination folder inside target repo" },
                "repo": { "type": "string", "description": "Target member repo (required when refresh=true)" },
                "type": { "type": "string", "description": "Default frontmatter type when source omits one" },
                "date": { "type": "string", "description": "Optional capture date (YYYY-MM-DD)" },
                "source": { "type": "string", "description": "Optional provenance label" },
                "allow_unresolved_links": { "type": "boolean", "default": false },
                "allow_ambiguous_links": { "type": "boolean", "default": false },
                "refresh": { "type": "boolean", "default": false, "description": "Overwrite existing target(s); requires SEM-02 prior read + expected_hash/expected_hashes" },
                "expected_hash": { "type": "string", "description": "CAS hash for single-target refresh" },
                "expected_hashes": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Map of target path -> CAS hash for multi-target refresh" },
                "read_receipt": { "type": "string", "description": "Stateless MCP 2.0 HMAC read_receipt for single-target refresh" },
                "read_receipts": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Map of target path -> read_receipt for multi-target refresh" },
                "dedup_threshold": { "type": "number", "description": "Optional BM25 score threshold to reject near-duplicates" },
                "dry_run": { "type": "boolean", "default": false },
                "request_id": { "type": "string", "description": "Optional idempotency key for ADR-0009 outcome registry" }
            }
        }),
        _ => json!({ "type": "object", "properties": {} }),
    };

    Arc::new(schema.as_object().expect("schema is a JSON object").clone())
}

/// Classify an error string into a machine-readable `(reason_code, recovery_hint)` pair.
#[must_use]
pub fn classify_error_with_hint(msg: &str) -> (&'static str, &'static str) {
    if msg.contains("policy/launch_grant") {
        (
            "launch_grant",
            "Restart '2brain mcp' with the required capability flag (--allow-mutations, --allow-composition, --allow-workspace-shape, --allow-ingest, or --allow-cognitive).",
        )
    } else if msg.contains("policy/revoked") {
        (
            "revoked",
            "Access to this repository is revoked or excluded by the active --policy file; choose an allowed member or update policy.",
        )
    } else if msg.contains("policy/egress") {
        (
            "egress_denied",
            "External egress is blocked by the active --policy file (AE24/R24); switch to a local destination or update egress_policy.",
        )
    } else if msg.contains("policy/prior_read") {
        (
            "prior_read",
            "Call tb_content on the target file without max_chars truncation, then retry with the returned hash (and read_receipt in stateless MCP 2.0).",
        )
    } else if msg.contains("policy/freshness") || msg.contains("expected_hash_mismatch") {
        (
            "expected_hash_mismatch",
            "Re-read the full file with tb_content to obtain its latest hash and merge your edit before retrying.",
        )
    } else if msg.contains("policy/path_scope") || msg.contains("symlink") {
        (
            "path_scope",
            "Use a workspace-confined relative POSIX path without '..' or escaping symlinks.",
        )
    } else if msg.contains("invalid_utf8") {
        (
            "invalid_utf8",
            "Repair or exclude the non-UTF-8 file via .2brain/ignore before indexing or editing.",
        )
    } else if msg.contains("snapshot_stale") {
        (
            "snapshot_stale",
            "The workspace changed while computing; retry the read immediately against the newer generation.",
        )
    } else if msg.contains("outcome_quota") {
        (
            "outcome_quota",
            "Prune/compact the outcome registry or raise quota with '2brain outcomes quota'.",
        )
    } else if msg.contains("outcome_conflict") {
        (
            "outcome_conflict",
            "Use a fresh 32-hex request_id for distinct arguments, or inspect the prior call via tb_mutation_outcome.",
        )
    } else if msg.contains("outcome_unknown") {
        (
            "outcome_unknown",
            "Do NOT retry blindly; inspect tb_mutation_outcome and tb_reconcile on the target path first.",
        )
    } else if msg.contains("policy/task_quota") {
        (
            "task_quota",
            "Wait for in-flight SEP-2663 tasks to finish or cancel an unneeded task via tasks/cancel.",
        )
    } else if msg.contains("already_exists") {
        (
            "already_exists",
            "Read the existing note via tb_content and update it with tb_write_file/tb_edit_file, or choose a new path.",
        )
    } else if msg.contains("not found") || msg.contains("not_found") {
        (
            "not_found",
            "Verify the repository and relative path with tb_members and tb_files.",
        )
    } else if msg.contains("policy/arguments") || msg.contains("invalid_arguments") {
        (
            "invalid_arguments",
            "Call tb_describe_tool(name) to inspect required parameters and valid value formats.",
        )
    } else {
        (
            "internal_error",
            "Inspect server diagnostics and verify workspace health with tb_overview and tb_diagnostics.",
        )
    }
}
