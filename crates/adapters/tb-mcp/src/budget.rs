//! Unified MCP response byte/token budget with 3-stage graceful degradation
//! (`MetaStrip` -> byte-dominant array binary-search trim -> honest truncation metadata).
//!
//! Keeps MCP tool payloads below typical client spill-to-disk thresholds (~50 KiB wire)
//! without corrupting JSON structure or misreporting truncated results as exhaustive.
//! Never applied to integrity-critical tools (`tb_content`, `tb_task_*`, `tb_run_procedure`,
//! `tb_mutation_outcome`, `tb_reconcile`, `tb_describe_tool`).

use serde_json::{Map, Value, json};

/// Default serialized JSON byte cap (~40 KiB so JSON-RPC envelope stays under ~50 KiB).
pub const DEFAULT_MAX_BYTES: usize = 40_000;

/// Conservative bytes-per-token factor for JSON payloads.
pub const AVG_BYTES_PER_TOKEN: usize = 4;

/// Effective response budget derived from optional `max_bytes` / `max_tokens` arguments.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResponseBudget {
    /// `None` means the caller explicitly opted out of byte/token capping (`<= 0`).
    pub max_bytes: Option<usize>,
}

impl Default for ResponseBudget {
    fn default() -> Self {
        Self {
            max_bytes: Some(DEFAULT_MAX_BYTES),
        }
    }
}

impl ResponseBudget {
    /// Compute effective budget from MCP tool arguments (`max_bytes` and/or `max_tokens`).
    ///
    /// - Passing positive `max_bytes` and/or `max_tokens` takes the tighter bound (min-merge).
    /// - Passing `<= 0` on one axis disables that axis; passing `<= 0` on all supplied axes
    ///   disables capping entirely.
    #[must_use]
    pub fn from_args(args: &Map<String, Value>) -> Self {
        let raw_bytes = args.get("max_bytes").and_then(Value::as_i64);
        let raw_tokens = args.get("max_tokens").and_then(Value::as_i64);
        if raw_bytes.is_none() && raw_tokens.is_none() {
            return Self::default();
        }
        let mut bound: Option<usize> = None;
        if let Some(b) = raw_bytes
            && b > 0
        {
            let ub = usize::try_from(b).unwrap_or(usize::MAX);
            bound = Some(bound.map_or(ub, |cur| cur.min(ub)));
        }
        if let Some(t) = raw_tokens
            && t > 0
        {
            let ut = usize::try_from(t)
                .unwrap_or(usize::MAX)
                .saturating_mul(AVG_BYTES_PER_TOKEN);
            bound = Some(bound.map_or(ut, |cur| cur.min(ut)));
        }
        Self { max_bytes: bound }
    }
}

/// Tools whose payloads carry fail-closed integrity state or full-read proofs (SEM-02 / D08 / D10)
/// and must NEVER be truncated by the generic response budgeter.
#[must_use]
pub fn is_budget_exempt_tool(name: &str) -> bool {
    matches!(
        name,
        "tb_content"
            | "tb_task_resume"
            | "tb_task_checkpoint"
            | "tb_task_list"
            | "tb_run_procedure"
            | "tb_mutation_outcome"
            | "tb_reconcile"
            | "tb_describe_tool"
    )
}

/// Apply 3-stage graceful degradation to a JSON tool payload when it exceeds `budget.max_bytes`.
///
/// Returns `(possibly_trimmed_value, was_truncated)`.
#[must_use]
pub fn apply_response_budget(mut payload: Value, budget: ResponseBudget) -> (Value, bool) {
    let Some(max_bytes) = budget.max_bytes else {
        return (payload, false);
    };
    if serialized_len(&payload) <= max_bytes {
        return (payload, false);
    }

    let Some(root) = payload.as_object_mut() else {
        return (payload, false);
    };

    // Locate the array occupying the most serialized bytes (top-level or 1 level nested),
    // so e.g. `context.fragments` (100 KiB) is trimmed rather than `context.related_neighbors` (400 B).
    let Some(target) = find_heaviest_array_path(root) else {
        return (payload, false);
    };

    let original_len = array_len_at(root, &target);
    if original_len == 0 {
        return (payload, false);
    }

    // Stage 1: MetaStrip — remove verbose optional keys (`excerpt`, `raw`) before dropping rows.
    // Preserves `snippet` (primary match line in `tb_grep`).
    let stripped = strip_verbose_fields_at(root, &target);
    if stripped {
        root.insert("_meta_stripped".to_owned(), Value::Bool(true));
        if serialized_len_obj(root) <= max_bytes {
            return (payload, true);
        }
    }

    // Stage 2: Binary search for the largest prefix of the target array whose exact serialized
    // payload INCLUDING all truncation metadata fields fits `<= max_bytes`.
    let full_items = clone_array_at(root, &target);
    let label = target.label();
    root.insert("_truncated_by_budget".to_owned(), Value::Bool(true));
    root.insert("_budget_bytes".to_owned(), json!(max_bytes));
    root.insert("_truncated_field".to_owned(), json!(label));
    root.insert("_original_count".to_owned(), json!(original_len));
    root.insert("_kept_count".to_owned(), json!(0));
    root.insert("count_is_exact".to_owned(), Value::Bool(false));

    let mut lo = 0usize;
    let mut hi = full_items.len();
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        set_array_prefix_at(root, &target, &full_items[..mid]);
        root.insert("_kept_count".to_owned(), json!(mid));
        if serialized_len_obj(root) <= max_bytes {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    set_array_prefix_at(root, &target, &full_items[..lo]);
    root.insert("_kept_count".to_owned(), json!(lo));

    // Keep pagination cursor consistent so callers never skip items `lo..original_len`.
    if root.contains_key("next_offset")
        && let Some(offset) = root.get("offset").and_then(Value::as_u64)
    {
        let next = offset.saturating_add(lo as u64);
        root.insert("next_offset".to_owned(), json!(next));
    }

    (payload, true)
}

#[derive(Clone, Debug)]
enum ArrayTarget {
    Top(String),
    Nested(String, String),
}

impl ArrayTarget {
    fn label(&self) -> String {
        match self {
            Self::Top(k) => k.clone(),
            Self::Nested(outer, inner) => format!("{outer}.{inner}"),
        }
    }
}

fn find_heaviest_array_path(root: &Map<String, Value>) -> Option<ArrayTarget> {
    let mut best: Option<(usize, String, ArrayTarget)> = None;
    for (k, v) in root {
        match v {
            Value::Array(arr) if !arr.is_empty() => {
                let bytes = serialized_len(v);
                let target = ArrayTarget::Top(k.clone());
                let label = target.label();
                if best
                    .as_ref()
                    .is_none_or(|(bb, blab, _)| bytes > *bb || (bytes == *bb && label < *blab))
                {
                    best = Some((bytes, label, target));
                }
            }
            Value::Object(inner) => {
                for (ik, iv) in inner {
                    if let Value::Array(arr) = iv
                        && !arr.is_empty()
                    {
                        let bytes = serialized_len(iv);
                        let target = ArrayTarget::Nested(k.clone(), ik.clone());
                        let label = target.label();
                        if best.as_ref().is_none_or(|(bb, blab, _)| {
                            bytes > *bb || (bytes == *bb && label < *blab)
                        }) {
                            best = Some((bytes, label, target));
                        }
                    }
                }
            }
            _ => {}
        }
    }
    best.map(|(_, _, t)| t)
}

fn array_mut<'a>(
    root: &'a mut Map<String, Value>,
    target: &ArrayTarget,
) -> Option<&'a mut Vec<Value>> {
    match target {
        ArrayTarget::Top(k) => root.get_mut(k).and_then(Value::as_array_mut),
        ArrayTarget::Nested(outer, inner) => root
            .get_mut(outer)
            .and_then(Value::as_object_mut)
            .and_then(|o| o.get_mut(inner))
            .and_then(Value::as_array_mut),
    }
}

fn array_len_at(root: &Map<String, Value>, target: &ArrayTarget) -> usize {
    match target {
        ArrayTarget::Top(k) => root.get(k).and_then(Value::as_array).map_or(0, Vec::len),
        ArrayTarget::Nested(outer, inner) => root
            .get(outer)
            .and_then(Value::as_object)
            .and_then(|o| o.get(inner))
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
    }
}

fn clone_array_at(root: &Map<String, Value>, target: &ArrayTarget) -> Vec<Value> {
    match target {
        ArrayTarget::Top(k) => root
            .get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        ArrayTarget::Nested(outer, inner) => root
            .get(outer)
            .and_then(Value::as_object)
            .and_then(|o| o.get(inner))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    }
}

fn set_array_prefix_at(root: &mut Map<String, Value>, target: &ArrayTarget, prefix: &[Value]) {
    if let Some(arr) = array_mut(root, target) {
        arr.clear();
        arr.extend_from_slice(prefix);
    }
}

fn strip_verbose_fields_at(root: &mut Map<String, Value>, target: &ArrayTarget) -> bool {
    let Some(arr) = array_mut(root, target) else {
        return false;
    };
    let mut removed_any = false;
    for item in arr.iter_mut() {
        if let Some(obj) = item.as_object_mut() {
            for key in ["excerpt", "raw"] {
                if obj.remove(key).is_some() {
                    removed_any = true;
                }
            }
        }
    }
    removed_any
}

fn serialized_len(v: &Value) -> usize {
    serde_json::to_string(v).map_or(usize::MAX, |s| s.len())
}

fn serialized_len_obj(m: &Map<String, Value>) -> usize {
    serde_json::to_string(m).map_or(usize::MAX, |s| s.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_from_args_min_merges_and_respects_opt_out() {
        let mut args = Map::new();
        assert_eq!(
            ResponseBudget::from_args(&args).max_bytes,
            Some(DEFAULT_MAX_BYTES)
        );

        args.insert("max_bytes".into(), json!(8000));
        args.insert("max_tokens".into(), json!(1000)); // 4000 bytes < 8000
        assert_eq!(ResponseBudget::from_args(&args).max_bytes, Some(4000));

        args.insert("max_bytes".into(), json!(0));
        args.insert("max_tokens".into(), json!(0));
        assert_eq!(ResponseBudget::from_args(&args).max_bytes, None);
    }

    #[test]
    fn budget_degrades_via_meta_strip_then_binary_search_and_updates_next_offset() {
        let items: Vec<Value> = (0..50)
            .map(|i| {
                json!({
                    "path": format!("notes/n{i:02}.md"),
                    "score": 10.0,
                    "excerpt": "x".repeat(200),
                })
            })
            .collect();
        let payload = json!({
            "search": { "matches": items },
            "offset": 10,
            "next_offset": 60,
            "total": 100
        });

        let (trimmed, was_trimmed) = apply_response_budget(
            payload,
            ResponseBudget {
                max_bytes: Some(1200),
            },
        );
        assert!(was_trimmed);
        assert_eq!(trimmed["_truncated_by_budget"], Value::Bool(true));
        assert_eq!(trimmed["_meta_stripped"], Value::Bool(true));
        assert_eq!(trimmed["_truncated_field"], json!("search.matches"));
        assert_eq!(trimmed["_original_count"], json!(50));
        let kept = trimmed["_kept_count"].as_u64().unwrap() as usize;
        assert!(kept > 0 && kept < 50);
        assert_eq!(trimmed["next_offset"], json!(10 + kept as u64));
        assert!(serde_json::to_string(&trimmed).unwrap().len() <= 1200);
    }

    #[test]
    fn budget_picks_heaviest_array_by_bytes_not_element_count() {
        let payload = json!({
            "context": {
                "related_neighbors": ["a.md", "b.md", "c.md", "d.md", "e.md", "f.md", "g.md", "h.md", "i.md", "j.md"],
                "fragments": [
                    { "path": "f1.md", "body": "A".repeat(800) },
                    { "path": "f2.md", "body": "B".repeat(800) },
                    { "path": "f3.md", "body": "C".repeat(800) }
                ]
            }
        });
        let (trimmed, was_trimmed) = apply_response_budget(
            payload,
            ResponseBudget {
                max_bytes: Some(1500),
            },
        );
        assert!(was_trimmed);
        assert_eq!(trimmed["_truncated_field"], json!("context.fragments"));
        assert_eq!(
            trimmed["context"]["related_neighbors"]
                .as_array()
                .unwrap()
                .len(),
            10,
            "tiny neighbor list must remain untouched while heavy fragments are trimmed"
        );
        assert!(serde_json::to_string(&trimmed).unwrap().len() <= 1500);
    }
}
