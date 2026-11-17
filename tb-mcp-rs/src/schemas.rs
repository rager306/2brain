//! The `outputSchema` each tool declares.
//!
//! A client validates `structuredContent` against these, so a field the adapter
//! adds to an answer without declaring it here is a protocol error at the client,
//! not a cosmetic mismatch. Every one is hand-written today; generating them from
//! the engine's `WIRE.md` is the next step, so the two cannot drift.

use std::sync::Arc;

use serde_json::{Value, json};

/// The output schema of a read: the engine's result object with its own key.
pub(crate) fn envelope_schema(verb: &'static str) -> Arc<serde_json::Map<String, Value>> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        verb.to_owned(),
        json!({"description": format!("the {verb} read's result")}),
    );
    // Paging metadata the adapter may add to any read's answer. A client
    // validates `structuredContent` against this schema, so a field the adapter
    // adds without declaring it here is a protocol error at the client, not a
    // cosmetic mismatch.
    for key in ["total", "offset"] {
        properties.insert(
            key.to_owned(),
            json!({"type": "integer", "description": format!("paging metadata added by the adapter ({key})")}),
        );
    }
    // The page that ends a sequence answers null here. A strict client validates the
    // answer against this declaration after the read has already succeeded, so a null
    // under an integer declaration is a protocol error, not a cosmetic mismatch.
    properties.insert(
        "next_offset".to_owned(),
        json!({"type": ["integer", "null"], "description": "paging metadata added by the adapter (next_offset); null on the page that ends the sequence"}),
    );
    Arc::new(
        json!({
            "type": "object",
            "properties": properties,
            "required": [verb],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_watch`: the delta the engine reported, or a timeout.
pub(crate) fn watch_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "watch": {
                    "type": "object",
                    "properties": {
                        "added": {"type": "array", "items": {"type": "string"}},
                        "removed": {"type": "array", "items": {"type": "string"}},
                        "modified": {"type": "array", "items": {"type": "string"}},
                        "version": {"type": ["integer", "null"]},
                        "timed_out": {"type": "boolean"}
                    },
                    "required": ["added", "removed", "modified", "timed_out"],
                    "additionalProperties": false
                }
            },
            "required": ["watch"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_rollback_preview`/// The output schema of `tb_rollback_preview`: a plan, path by path.
pub(crate) fn rollback_preview_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "rollback_preview": {
                    "type": "object",
                    "properties": {
                        "repo": {"type": "string"},
                        "commit": {"type": "string"},
                        "subject": {"type": "string"},
                        "parent": {"type": ["string", "null"]},
                        "paths": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": {"type": "string"},
                                    "action": {"type": "string", "enum": ["restore", "delete"]},
                                    "state": {"type": "string", "enum": ["clean", "dirty", "changed_since"]}
                                },
                                "required": ["path", "action", "state"],
                                "additionalProperties": false
                            }
                        },
                        "safe": {"type": "boolean"}
                    },
                    "required": ["repo", "commit", "subject", "parent", "paths", "safe"],
                    "additionalProperties": false
                }
            },
            "required": ["rollback_preview"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_graph_health`/// The output schema of `tb_graph_health`: the report and its bounded lists.
pub(crate) fn graph_health_schema() -> Arc<serde_json::Map<String, Value>> {
    let node = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "in": {"type": "integer"},
            "out": {"type": "integer"},
            "total": {"type": "integer"}
        },
        "required": ["path", "in", "out", "total"],
        "additionalProperties": false
    });
    let report = json!({
        "type": "object",
        "properties": {
            "repo": {"type": ["string", "null"]},
            "scope": {"type": "string"},
            "counts": {"type": "object"},
            "cycles": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "length": {"type": "integer"},
                        "nodes": {"type": "array", "items": {"type": "string"}}
                    },
                    "required": ["length", "nodes"],
                    "additionalProperties": false
                }
            },
            "isolated": {"type": "array", "items": node},
            "dead_ends": {"type": "array", "items": node},
            "sources": {"type": "array", "items": node},
            "broken_links": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "code": {"type": "string"},
                        "message": {"type": "string"},
                        "line": {"type": ["integer", "null"]}
                    },
                    "required": ["path", "code", "message"],
                    "additionalProperties": false
                }
            },
            "bridges": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "a": {"type": "string"},
                        "b": {"type": "string"}
                    },
                    "required": ["a", "b"],
                    "additionalProperties": false
                }
            },
            "articulation_points": {"type": "array", "items": node},
            "clustering": {
                "type": "object",
                "properties": {
                    "triangles": {"type": "integer"},
                    "transitivity": {"type": "number"}
                },
                "required": ["triangles", "transitivity"],
                "additionalProperties": false
            },
            "central": {"type": "array", "items": node},
            "truncated": {"type": "boolean"}
        },
        "required": ["counts", "cycles", "isolated", "dead_ends", "sources", "broken_links", "bridges", "articulation_points", "clustering", "central", "truncated"],
        "additionalProperties": false
    });
    Arc::new(
        json!({
            "type": "object",
            "properties": {"graph_health": report},
            "required": ["graph_health"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_batch`: the per-op results and the saga's commit./// The output schema of `tb_batch`: the per-op results and the saga's commit.
pub(crate) fn batch_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "ops": {
                    "type": "array",
                    "description": "one entry per applied operation, in order",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {"type": "string"},
                            "path": {"type": "string"},
                            "hash": {"type": ["string", "null"]}
                        },
                        "required": ["op", "path"],
                        "additionalProperties": false
                    }
                },
                "reflected": {"type": "boolean"},
                "commit": {},
                "commits": commits_object(),
                "diagnostics": {"type": "array"},
                "untracked_files": {"type": "array"}
            },
            "required": ["ops", "commit", "commits", "diagnostics"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_subtypes`: the base it was asked about plus the
/// resolved list, which the engine returns as two keys rather than one envelope.
pub(crate) fn subtypes_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "base": {"type": "string", "description": "the base the answer is about"},
                "subtypes": {"type": "array", "description": "direct and transitive subtypes"}
            },
            "required": ["base", "subtypes"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_reconcile`: a flat verdict, not an envelope. The
/// engine answers `outcome` with the current hash and commit beside it.
pub(crate) fn reconcile_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "outcome": {
                    "type": "string",
                    "enum": ["applied", "not_applied"],
                    "description": "whether the interrupted write landed"
                },
                "current_hash": {
                    "type": ["string", "null"],
                    "description": "the file's hash now; null when it does not exist"
                },
                "current_commit": {
                    "type": ["string", "null"],
                    "description": "HEAD of the owning repo now; null off-git"
                }
            },
            "required": ["outcome", "current_hash", "current_commit"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_grep`: one page of matches plus the totals that make
/// the page interpretable.
pub(crate) fn grep_schema() -> Arc<serde_json::Map<String, Value>> {
    let hit = json!({
        "type": "object",
        "properties": {
            "path": {"type": "string"},
            "repo": {"type": ["string", "null"]},
            "line": {"type": "integer", "description": "1-based line number"},
            "column": {"type": "integer", "description": "1-based character column of the match"},
            "text": {"type": "string", "description": "the line, cut at 400 characters"},
            "text_truncated": {"type": "boolean"},
        },
        "required": ["path", "line", "column", "text", "text_truncated"],
        "additionalProperties": false,
    });
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "grep": {
                    "type": "object",
                    "properties": {
                        "matches": {"type": "array", "items": hit},
                        "total": {"type": "integer"},
                        "truncated": {"type": "boolean"},
                        "skipped_files": {"type": "integer"},
                    },
                    "required": ["matches", "total", "truncated", "skipped_files"],
                    "additionalProperties": false,
                }
            },
            "required": ["grep"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_instances`: the engine's envelope plus paging.
pub(crate) fn instances_schema() -> Arc<serde_json::Map<String, Value>> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "instances".to_owned(),
        json!({"type": "array", "description": "resolved instances"}),
    );
    properties.insert(
        "count".to_owned(),
        json!({"type": "integer", "description": "instances the engine parsed"}),
    );
    properties.insert(
        "aborted_at_load".to_owned(),
        json!({"type": "boolean", "description": "a broken vocabulary skipped validation"}),
    );
    for key in ["total", "offset", "next_offset"] {
        properties.insert(
            key.to_owned(),
            json!({"type": ["integer", "null"], "description": format!("paging metadata added by the adapter ({key})")}),
        );
    }
    Arc::new(
        json!({
            "type": "object",
            "properties": properties,
            "required": ["instances"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_candidates`: the scan's envelope.
pub(crate) fn candidates_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "candidates": {"type": "array", "description": "one entry per scanned file"},
                "aborted_at_load": {"type": "boolean"}
            },
            "required": ["candidates"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_references`: either or both directions.
pub(crate) fn references_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "references_out": {"type": "array", "description": "edges the file authors"},
                "references_in": {"type": "array", "description": "edges pointing at the file"}
            },
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The engine answers a mutation with `commits` as an OBJECT — `{repo: sha}`, one
/// entry per touched member — and declaring it as an array made every successful
/// mutation reply invalid at a strict client, while the mutation itself had landed.
/// Found by a live Hermes session on 2026-11-15: the write succeeded and the answer
/// came back as "Invalid structured content". `tests/schema_contract.rs` pins it
/// against payloads captured from a real engine.
fn commits_object() -> Value {
    json!({
        "type": "object",
        "additionalProperties": {"type": "string"},
        "description": "one entry per touched member: repo -> commit",
    })
}

/// The output schema shared by every mutation: the engine's mutation result.
pub(crate) fn mutation_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "absolute path that was written"},
                // Null after a delete: a deleted file has no content hash, and declaring
                // this as a plain string made every successful delete invalid at a strict
                // client — the same class as `commits`, found by a live session on
                // 2026-11-15 immediately after that one was fixed.
                "hash": {
                    "type": ["string", "null"],
                    "description": "content hash after the mutation; null after a delete",
                },
                "last_live_commit": {
                    "type": ["string", "null"],
                    "description": "the commit the deleted file last existed in, null for writes",
                },
                // `tb_assign_block_id` answers with the block it named and the reference
                // that addresses it; every other mutation omits both. Declared because
                // the schema is the client's contract, not a hint.
                "id": {
                    "type": ["string", "null"],
                    "description": "the block id, for tb_assign_block_id",
                },
                "ref": {
                    "type": ["string", "null"],
                    "description": "the reference that addresses the block, for tb_assign_block_id",
                },
                "reflected": {"type": "boolean"},
                "commit": {},
                "commits": commits_object(),
                "diagnostics": {"type": "array"},
                "untracked_files": {"type": "array"}
            },
            "required": ["path"],
            "additionalProperties": true,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_follow`.
pub(crate) fn follow_schema() -> Arc<serde_json::Map<String, Value>> {
    let mut properties = serde_json::Map::new();
    properties.insert(
        "follow".to_owned(),
        json!({
            "type": "object",
            "properties": {
                "target": {"type": "string"},
                "origin": {"type": "string"},
                "resolved": {"type": "boolean"},
                "referent": {"type": "boolean"},
                "snippet": {"type": "string"}
            },
            "required": ["target", "origin", "resolved"],
            "additionalProperties": true
        }),
    );
    Arc::new(
        json!({
            "type": "object",
            "properties": properties,
            "required": ["follow"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}

/// The output schema of `tb_typed`.
pub(crate) fn typed_schema() -> Arc<serde_json::Map<String, Value>> {
    Arc::new(
        json!({
            "type": "object",
            "properties": {
                "typed": {"type": "boolean"},
                "kind": {"description": "instance, type-def, unclassified, or null"},
                "path": {"type": "string"},
                "claim": {"type": "array"},
                "resolves": {"type": "boolean"},
                "closure": {"type": "array"}
            },
            "required": ["typed", "path"],
            "additionalProperties": false,
        })
        .as_object()
        .expect("object schema")
        .clone(),
    )
}
