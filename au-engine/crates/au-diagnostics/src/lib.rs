//! Structured diagnostic records with stable codes, spans, and suggested-fix slots.
//!
//! `DiagnosticCode` is a newtype around `&'static str` so codes are added as
//! module-level constants from each producing crate without enum churn. Stable
//! string codes are kebab-case, e.g. `reserved-key-location`.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A stable diagnostic identifier. The backing string is the contract;
/// downstream consumers (LSP, CI gate, formatters) match on the string. Codes
/// are kebab-case.
///
/// Producers declare codes as module-level constants:
/// ```no_run
/// use au_diagnostics::DiagnosticCode;
/// pub const RESERVED_KEY_LOCATION: DiagnosticCode =
///     DiagnosticCode::from_static("reserved-key-location");
/// ```
///
/// Backed by `Cow<'static, str>` so producer-defined codes are zero-cost
/// (`Borrowed`) while deserialized codes from JSON allocate owned strings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DiagnosticCode(pub Cow<'static, str>);

impl DiagnosticCode {
    pub const fn from_static(code: &'static str) -> Self {
        Self(Cow::Borrowed(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The weight of a diagnostic. Part of the wire contract.
///
/// `Error` is the only blocking level: it prevents a downstream stage from
/// running (graph-load errors block per-file validation; per-file errors stay
/// per-file). Every other level is advisory — the stage completes and the
/// issue is surfaced. Consumers match on the serialized string, not the
/// declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks a downstream stage.
    Error,
    /// Advisory, high attention. Something has diverged from a version it was
    /// reconciled against, e.g. a pinned reference's live counterpart or a
    /// shadow of a hardwired engine type. Never blocks: the local copy still
    /// validates standalone. Its own tier so consumers can rank it above
    /// ordinary warnings; reusable for other staleness conditions.
    Drift,
    /// Advisory. The engine completes the stage and surfaces the issue.
    Warning,
    /// Advisory, a suggestion.
    Hint,
}

/// Half-open byte range into a single source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ByteRange {
    pub start: usize,
    pub end: usize,
}

impl ByteRange {
    pub const fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// A 1-based line/column position. `col` counts UTF-8 bytes from the line
/// start plus one, deterministic regardless of encoding width; consumers
/// needing character or UTF-16 columns convert within the one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineCol {
    pub line: usize,
    pub col: usize,
}

/// The line/column rendering of a `ByteRange`, both endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineColRange {
    pub start: LineCol,
    pub end: LineCol,
}

/// Byte-offset → line/column conversion table for one file's content.
///
/// Holds the byte offset of each line start (lines split at `\n` only, so a
/// `\r` counts into the column). Built once per read file, O(log lines) per
/// lookup. Works on raw bytes, so non-UTF-8 files index fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineIndex {
    /// Byte offset of each line's first byte; `[0]` is always 0.
    line_starts: Vec<usize>,
}

impl LineIndex {
    pub fn new(bytes: &[u8]) -> Self {
        let mut line_starts = vec![0];
        line_starts.extend(
            bytes
                .iter()
                .enumerate()
                .filter(|(_, b)| **b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self { line_starts }
    }

    /// The 1-based line/column of a byte offset. An offset past the end of
    /// the content clamps into the last line.
    pub fn line_col(&self, offset: usize) -> LineCol {
        let line_idx = self
            .line_starts
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);
        LineCol {
            line: line_idx + 1,
            col: offset - self.line_starts[line_idx] + 1,
        }
    }

    /// Both endpoints of a byte range as line/column.
    pub fn line_col_range(&self, range: ByteRange) -> LineColRange {
        LineColRange {
            start: self.line_col(range.start),
            end: self.line_col(range.end),
        }
    }
}

/// A span pins a `ByteRange` to a specific source file. The byte range is
/// canonical; `line_col` is its derived rendering, attached by the engine
/// build once the file's `LineIndex` exists, so line-oriented consumers
/// don't re-read the file to correlate spans.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub file: PathBuf,
    pub range: ByteRange,
    /// `None` until attached, and for spans into files the build never read
    /// (assets, unreadable files). Always served for read files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_col: Option<LineColRange>,
}

impl Span {
    pub fn new(file: impl Into<PathBuf>, range: ByteRange) -> Self {
        Self {
            file: file.into(),
            range,
            line_col: None,
        }
    }

    /// Span targeting a file as a whole, with no specific byte position.
    /// Renders as `file:@0` in the human formatter; suitable for diagnostics
    /// that aren't tied to a particular location (knowledge-base-load advisories,
    /// "this file exists but..." references).
    pub fn for_file(file: impl Into<PathBuf>) -> Self {
        Self::new(file, ByteRange::new(0, 0))
    }

    /// Attach the line/column rendering derived from the file's index.
    pub fn attach_line_col(&mut self, index: &LineIndex) {
        self.line_col = Some(index.line_col_range(self.range));
    }
}

/// A suggested fix for a diagnostic: a human-readable `description`, plus an
/// optional list of machine-applicable `actions`.
///
/// `description` is always the human label and the advisory fallback. `actions`
/// is empty for an advisory-only fix (the common case), and non-empty when the
/// engine can hand a consumer enough to RUN a mutation without parsing the
/// message or slicing the span. It is a LIST so a fix can offer a CHOICE of
/// alternatives (e.g. pick among candidate rename targets); a single action is a
/// length-1 list.
///
/// The serde shape is the wire contract, `Diagnostic` serializes directly. The
/// `actions` field is `skip_serializing_if` empty, so an advisory-only fix
/// serializes byte-identically to the description-only form: adding `actions`
/// is ADDITIVE, no `schema_version` bump. See `crates/au-engine/WIRE.md`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SuggestedFix {
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<FixAction>,
}

impl SuggestedFix {
    /// An advisory-only fix: a description with no machine-applicable action.
    pub fn advisory(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            actions: Vec::new(),
        }
    }

    /// A machine-applicable fix: a description plus one or more actions a
    /// consumer can run.
    pub fn with_actions(description: impl Into<String>, actions: Vec<FixAction>) -> Self {
        Self {
            description: description.into(),
            actions,
        }
    }
}

/// One machine-applicable action a consumer can run to resolve a diagnostic.
///
/// `verb` names a mutation from the engine's own closed catalog (e.g. `add_dep`),
/// so a generic consumer runs `mutate({ verb, ...args, ...topology })` with no
/// per-code switch. `args` carries the args the ENGINE parsed, keyed by the
/// verb's own argument names so they splat straight into the mutation call. Args
/// are PARTIAL by design: the engine supplies what it parsed (e.g. the `::repo`
/// qualifier), and the consumer completes workspace-topology args (e.g. `root`,
/// derived from `resolve_member`), which are not parsed source.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FixAction {
    /// The mutation verb this action runs, from the engine's closed catalog.
    pub verb: String,
    /// This action's human-readable label.
    pub title: String,
    /// The engine-supplied args, keyed by the verb's own argument names. Empty
    /// when the verb needs only consumer-supplied topology args.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub args: BTreeMap<String, String>,
}

impl FixAction {
    /// An `edit_file` action: an exact string replacement in `path`.
    ///
    /// Fully self-contained, the consumer runs `mutate(edit_file, args)` as-is.
    /// `old_string` must match UNIQUELY in the file or `edit_file` rejects, which
    /// is the safety net: an ambiguous match (a substring, or a repeated token)
    /// rejects cleanly and the consumer falls back to the advisory `description`,
    /// never a wrong-place edit. A caller reconstructs `old_string` from the
    /// structure it parsed, no source read required.
    pub fn edit_file(
        path: impl Into<String>,
        old_string: impl Into<String>,
        new_string: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            verb: "edit_file".to_string(),
            title: title.into(),
            args: BTreeMap::from([
                ("path".to_string(), path.into()),
                ("old_string".to_string(), old_string.into()),
                ("new_string".to_string(), new_string.into()),
            ]),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub span: Span,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related: Vec<Span>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<SuggestedFix>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_CODE: DiagnosticCode = DiagnosticCode::from_static("test-code");

    #[test]
    fn round_trips_through_json() {
        let diag = Diagnostic {
            code: TEST_CODE.clone(),
            severity: Severity::Error,
            span: Span::new("foo.yaml", ByteRange::new(0, 5)),
            message: "bad".into(),
            related: vec![],
            fix: None,
        };

        let json = serde_json::to_string(&diag).unwrap();
        let back: Diagnostic = serde_json::from_str(&json).unwrap();
        assert_eq!(diag, back);
    }

    #[test]
    fn omits_empty_related_and_fix_in_json() {
        let diag = Diagnostic {
            code: TEST_CODE.clone(),
            severity: Severity::Warning,
            span: Span::new("foo.yaml", ByteRange::new(0, 1)),
            message: "x".into(),
            related: vec![],
            fix: None,
        };
        let json = serde_json::to_string(&diag).unwrap();
        assert!(!json.contains("related"));
        assert!(!json.contains("fix"));
    }

    #[test]
    fn advisory_fix_serializes_without_an_actions_key() {
        // An advisory-only fix carries no `actions`, so it serializes exactly like
        // the description-only form: adding `actions` is ADDITIVE on the wire.
        let fix = SuggestedFix::advisory("do the thing");
        let json = serde_json::to_string(&fix).unwrap();
        assert_eq!(json, r#"{"description":"do the thing"}"#);
        let back: SuggestedFix = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fix);
        assert!(back.actions.is_empty());
    }

    #[test]
    fn a_multi_action_fix_serializes_as_a_list_and_round_trips() {
        // The `actions` field is a LIST so a fix can offer a CHOICE of N
        // alternatives (the tier-3 multi-option shape). Here two `edit_file`
        // actions, e.g. the disambiguation of an ambiguous wikilink target.
        let fix = SuggestedFix::with_actions(
            "resolve the ambiguous link",
            vec![
                FixAction::edit_file("n.md", "[[note]]", "[[a/note]]", "Point at a/note"),
                FixAction::edit_file("n.md", "[[note]]", "[[b/note]]", "Point at b/note"),
            ],
        );
        let json = serde_json::to_value(&fix).unwrap();
        assert_eq!(
            json["actions"].as_array().map(|a| a.len()),
            Some(2),
            "actions serializes as a JSON array of N"
        );
        assert_eq!(json["actions"][0]["args"]["new_string"], "[[a/note]]");
        assert_eq!(json["actions"][1]["args"]["new_string"], "[[b/note]]");
        let back: SuggestedFix = serde_json::from_value(json).unwrap();
        assert_eq!(back, fix);
        assert_eq!(back.actions.len(), 2);
    }

    #[test]
    fn code_is_serialized_as_bare_string() {
        let json = serde_json::to_string(&TEST_CODE).unwrap();
        assert_eq!(json, "\"test-code\"");
    }

    #[test]
    fn code_round_trips_to_owned_string() {
        let json = serde_json::to_string(&TEST_CODE).unwrap();
        let back: DiagnosticCode = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_str(), "test-code");
    }

    #[test]
    fn line_index_maps_offsets_to_one_based_line_col() {
        let idx = LineIndex::new(b"ab\ncde\n\nf");
        assert_eq!(idx.line_col(0), LineCol { line: 1, col: 1 });
        assert_eq!(idx.line_col(2), LineCol { line: 1, col: 3 }); // the \n itself
        assert_eq!(idx.line_col(3), LineCol { line: 2, col: 1 });
        assert_eq!(idx.line_col(7), LineCol { line: 3, col: 1 }); // empty line
        assert_eq!(idx.line_col(8), LineCol { line: 4, col: 1 });
    }

    #[test]
    fn line_index_clamps_past_end_into_last_line() {
        let idx = LineIndex::new(b"ab\ncd");
        assert_eq!(idx.line_col(99), LineCol { line: 2, col: 97 });
    }

    #[test]
    fn line_index_counts_columns_in_bytes() {
        // 'é' is two UTF-8 bytes; col counts bytes, not chars.
        let idx = LineIndex::new("é x".as_bytes());
        assert_eq!(idx.line_col(2), LineCol { line: 1, col: 3 });
    }

    #[test]
    fn line_index_on_empty_content() {
        let idx = LineIndex::new(b"");
        assert_eq!(idx.line_col(0), LineCol { line: 1, col: 1 });
    }

    #[test]
    fn span_serializes_line_col_only_when_attached() {
        let mut span = Span::new("foo.md", ByteRange::new(3, 5));
        let json = serde_json::to_string(&span).unwrap();
        assert!(!json.contains("line_col"));

        span.attach_line_col(&LineIndex::new(b"ab\ncde"));
        let json = serde_json::to_string(&span).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["line_col"]["start"]["line"], 2);
        assert_eq!(v["line_col"]["start"]["col"], 1);
        assert_eq!(v["line_col"]["end"]["line"], 2);
        assert_eq!(v["line_col"]["end"]["col"], 3);
    }
}
