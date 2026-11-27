//! Pure Markdown, YAML frontmatter, wikilink, anchor, and EN/RU tokenizer (0 I/O).
//!
//! Built strictly from `spec/01-workspace-and-syntax.md`.

use std::collections::BTreeMap;

use memchr::memmem;
use saphyr::{LoadableYamlNode, Scalar, Yaml};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Edge semantic classification (`KTD6` — never conflated).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// Wikilink inside a typed frontmatter field value.
    Field,
    /// Body wikilink carrying `:field` (`[[target:grounds]]`).
    Contributing,
    /// Plain prose wikilink in the markdown body.
    Navigational,
}

impl EdgeKind {
    #[must_use]
    pub const fn is_structural(self) -> bool {
        matches!(self, Self::Field | Self::Contributing)
    }
}

/// Lossless frontmatter value representation (YAML 1.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FrontmatterValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Seq(Vec<Self>),
    Map(BTreeMap<String, Self>),
}

impl FrontmatterValue {
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

/// A parsed `[[target(::repo)?(#anchor)?(:field)?(|alias)?]]` wikilink.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WikiLink {
    pub raw: String,
    pub target: String,
    pub repo: Option<String>,
    pub anchor: Option<String>,
    pub field: Option<String>,
    pub alias: Option<String>,
    pub kind: EdgeKind,
    pub byte_start: usize,
    pub byte_end: usize,
    pub line: usize,
}

/// Addressable heading or `^block-id` anchor inside a document.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Anchor {
    Heading {
        level: u8,
        text: String,
        slug: String,
        byte_start: usize,
        byte_end: usize,
        line: usize,
    },
    Block {
        id: String,
        byte_start: usize,
        byte_end: usize,
        line: usize,
    },
}

/// Lowercased EN/RU token with byte offsets for proximity scoring and excerpts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenSpan {
    pub term: String,
    pub position: u32,
    pub byte_start: usize,
    pub byte_end: usize,
}

/// Result of parsing one Markdown document strictly from UTF-8 bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct ParsedDocument {
    pub frontmatter: Option<BTreeMap<String, FrontmatterValue>>,
    pub frontmatter_error: Option<String>,
    pub declared_type: Option<String>,
    pub description: Option<String>,
    pub body_offset: usize,
    pub wikilinks: Vec<WikiLink>,
    pub anchors: Vec<Anchor>,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum SyntaxError {
    #[error("file contains invalid UTF-8 at byte {valid_up_to}")]
    InvalidUtf8 { valid_up_to: usize },
}

/// Strictly parse UTF-8 document bytes per `spec/01-workspace-and-syntax.md`.
pub fn parse_document(bytes: &[u8]) -> Result<ParsedDocument, SyntaxError> {
    let text = std::str::from_utf8(bytes).map_err(|e| SyntaxError::InvalidUtf8 {
        valid_up_to: e.valid_up_to(),
    })?;

    let (frontmatter, frontmatter_error, body_offset, body_start_line) = parse_frontmatter(text);
    let mut wikilinks = Vec::new();
    let mut declared_type = None;
    let mut description = None;

    if let Some(ref fm) = frontmatter {
        declared_type = fm
            .get("type")
            .and_then(FrontmatterValue::as_str)
            .map(str::to_owned);
        description = fm
            .get("description")
            .and_then(FrontmatterValue::as_str)
            .map(str::to_owned);
        for (field_name, value) in fm {
            collect_frontmatter_links(field_name, value, &mut wikilinks);
        }
    }

    let body = &text[body_offset..];
    let mut anchors = Vec::new();
    scan_body(
        body,
        body_offset,
        body_start_line,
        &mut wikilinks,
        &mut anchors,
    );

    Ok(ParsedDocument {
        frontmatter,
        frontmatter_error,
        declared_type,
        description,
        body_offset,
        wikilinks,
        anchors,
    })
}

/// Tokenize text per Spec §4: maximal Unicode alphanumeric runs, lowercased,
/// no stemming, no stopword deletion (EN + RU parity).
#[must_use]
pub fn tokenize(text: &str) -> Vec<TokenSpan> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut pos: u32 = 0;

    for (idx, ch) in text.char_indices() {
        if ch.is_alphanumeric() {
            if start.is_none() {
                start = Some(idx);
            }
        } else if let Some(s) = start.take() {
            out.push(TokenSpan {
                term: text[s..idx].to_lowercase(),
                position: pos,
                byte_start: s,
                byte_end: idx,
            });
            pos = pos.saturating_add(1);
        }
    }
    if let Some(s) = start {
        out.push(TokenSpan {
            term: text[s..].to_lowercase(),
            position: pos,
            byte_start: s,
            byte_end: text.len(),
        });
    }
    out
}

/// Deterministic URL-safe heading slug (lowercase alphanumerics + hyphens).
#[must_use]
pub fn heading_slug(text: &str) -> String {
    let mut slug = String::with_capacity(text.len());
    let mut prev_dash = true;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                slug.push(lc);
            }
            prev_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn parse_frontmatter(
    text: &str,
) -> (
    Option<BTreeMap<String, FrontmatterValue>>,
    Option<String>,
    usize,
    usize,
) {
    let after_open = if text.starts_with("---\n") {
        4
    } else if text.starts_with("---\r\n") {
        5
    } else {
        return (None, None, 0, 1);
    };

    let mut offset = after_open;
    let mut line_no = 2usize;
    let bytes = text.as_bytes();
    while offset <= bytes.len() {
        let line_end =
            memchr::memchr(b'\n', &bytes[offset..]).map_or(bytes.len(), |rel| offset + rel);
        let raw_line = &text[offset..line_end];
        let trimmed = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if trimmed == "---" {
            let yaml_src = &text[after_open..offset];
            let body_offset = if line_end < bytes.len() {
                line_end + 1
            } else {
                line_end
            };
            let body_line = line_no + 1;
            if yaml_src.trim().is_empty() {
                return (Some(BTreeMap::new()), None, body_offset, body_line);
            }
            match Yaml::load_from_str(yaml_src) {
                Ok(docs) => match docs.into_iter().next() {
                    Some(Yaml::Mapping(map)) => {
                        let converted = convert_yaml_mapping(map);
                        return (Some(converted), None, body_offset, body_line);
                    }
                    Some(Yaml::Value(Scalar::Null)) | None => {
                        return (Some(BTreeMap::new()), None, body_offset, body_line);
                    }
                    Some(_) => {
                        return (
                            None,
                            Some("frontmatter root must be a YAML mapping".to_owned()),
                            body_offset,
                            body_line,
                        );
                    }
                },
                Err(err) => {
                    return (
                        None,
                        Some(format!("malformed YAML frontmatter: {err}")),
                        body_offset,
                        body_line,
                    );
                }
            }
        }
        if line_end == bytes.len() {
            break;
        }
        offset = line_end + 1;
        line_no += 1;
    }

    (None, None, 0, 1)
}

fn convert_yaml_mapping(map: saphyr::Mapping<'_>) -> BTreeMap<String, FrontmatterValue> {
    let mut out = BTreeMap::new();
    for (k, v) in map {
        let key = match k {
            Yaml::Value(Scalar::String(s)) => s.into_owned(),
            Yaml::Value(Scalar::Integer(i)) => i.to_string(),
            Yaml::Value(Scalar::Boolean(b)) => b.to_string(),
            other => format!("{other:?}"),
        };
        out.insert(key, convert_yaml_value(v));
    }
    out
}

fn convert_yaml_value(val: Yaml<'_>) -> FrontmatterValue {
    match val {
        Yaml::Value(Scalar::Null) => FrontmatterValue::Null,
        Yaml::Value(Scalar::Boolean(b)) => FrontmatterValue::Bool(b),
        Yaml::Value(Scalar::Integer(i)) => FrontmatterValue::Int(i),
        Yaml::Value(Scalar::FloatingPoint(f)) => FrontmatterValue::Float(f.into_inner()),
        Yaml::Value(Scalar::String(s)) => FrontmatterValue::String(s.into_owned()),
        Yaml::Sequence(seq) => {
            FrontmatterValue::Seq(seq.into_iter().map(convert_yaml_value).collect())
        }
        Yaml::Mapping(map) => FrontmatterValue::Map(convert_yaml_mapping(map)),
        Yaml::Representation(s, _, _) => FrontmatterValue::String(s.into_owned()),
        Yaml::Tagged(_, inner) => convert_yaml_value(*inner),
        Yaml::Alias(_) | Yaml::BadValue => FrontmatterValue::Null,
    }
}

fn collect_frontmatter_links(field_name: &str, value: &FrontmatterValue, out: &mut Vec<WikiLink>) {
    match value {
        FrontmatterValue::String(s) => {
            let finder = memmem::Finder::new(b"[[");
            let mut pos = 0usize;
            while let Some(rel) = finder.find(&s.as_bytes()[pos..]) {
                let start = pos + rel;
                if let Some(close_rel) = memmem::find(&s.as_bytes()[start + 2..], b"]]") {
                    let end = start + 2 + close_rel + 2;
                    let inner = &s[start + 2..start + 2 + close_rel];
                    if let Some(mut link) = parse_wikilink_inner(inner, EdgeKind::Field, 0, 0, 1) {
                        link.raw = s[start..end].to_owned();
                        if link.field.is_none() {
                            link.field = Some(field_name.to_owned());
                        }
                        out.push(link);
                    }
                    pos = end;
                } else {
                    break;
                }
            }
        }
        FrontmatterValue::Seq(items) => {
            for item in items {
                collect_frontmatter_links(field_name, item, out);
            }
        }
        FrontmatterValue::Map(map) => {
            for (k, v) in map {
                collect_frontmatter_links(k, v, out);
            }
        }
        FrontmatterValue::Null
        | FrontmatterValue::Bool(_)
        | FrontmatterValue::Int(_)
        | FrontmatterValue::Float(_) => {}
    }
}

fn scan_body(
    body: &str,
    base_offset: usize,
    start_line: usize,
    wikilinks: &mut Vec<WikiLink>,
    anchors: &mut Vec<Anchor>,
) {
    let bytes = body.as_bytes();
    let mut offset = 0usize;
    let mut line_no = start_line;
    let mut fence_char: Option<(u8, usize)> = None;

    while offset < bytes.len() {
        let line_end =
            memchr::memchr(b'\n', &bytes[offset..]).map_or(bytes.len(), |rel| offset + rel);
        let raw_line = &body[offset..line_end];
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        let abs_start = base_offset + offset;

        if let Some((fc, flen)) = fence_char {
            if is_closing_fence(line, fc, flen) {
                fence_char = None;
            }
        } else if let Some((fc, flen)) = parse_opening_fence(line) {
            fence_char = Some((fc, flen));
        } else {
            if let Some((level, heading_text)) = parse_atx_heading(line) {
                anchors.push(Anchor::Heading {
                    level,
                    slug: heading_slug(heading_text),
                    text: heading_text.to_owned(),
                    byte_start: abs_start,
                    byte_end: abs_start + line.len(),
                    line: line_no,
                });
            }
            if let Some((id, id_start, id_end)) = parse_trailing_block_id(line) {
                anchors.push(Anchor::Block {
                    id: id.to_owned(),
                    byte_start: abs_start + id_start,
                    byte_end: abs_start + id_end,
                    line: line_no,
                });
            }
            scan_line_wikilinks(line, abs_start, line_no, wikilinks);
        }

        if line_end == bytes.len() {
            break;
        }
        offset = line_end + 1;
        line_no += 1;
    }
}

fn parse_opening_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line.trim_start_matches(' ');
    let leading_spaces = line.len() - trimmed.len();
    if leading_spaces > 3 {
        return None;
    }
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count < 3 {
        return None;
    }
    if first == b'`' && trimmed[count..].contains('`') {
        return None;
    }
    Some((first, count))
}

fn is_closing_fence(line: &str, fence_char: u8, min_len: usize) -> bool {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return false;
    }
    let count = trimmed.bytes().take_while(|&b| b == fence_char).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn parse_atx_heading(line: &str) -> Option<(u8, &str)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    if text.is_empty() {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "hashes is bounded to 1..=6"
    )]
    Some((hashes as u8, text))
}

fn parse_trailing_block_id(line: &str) -> Option<(&str, usize, usize)> {
    let trimmed_end = line.trim_end_matches([' ', '\t']);
    let caret_pos = trimmed_end.rfind('^')?;
    if caret_pos == 0 {
        return None;
    }
    let prev = trimmed_end.as_bytes()[caret_pos - 1];
    if prev != b' ' && prev != b'\t' {
        return None;
    }
    let id = &trimmed_end[caret_pos + 1..];
    if id.is_empty()
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    Some((id, caret_pos, trimmed_end.len()))
}

fn scan_line_wikilinks(line: &str, line_abs_start: usize, line_no: usize, out: &mut Vec<WikiLink>) {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut inline_code_ticks: Option<usize> = None;

    while i < bytes.len() {
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            match inline_code_ticks {
                Some(open) if open == ticks => inline_code_ticks = None,
                None => inline_code_ticks = Some(ticks),
                Some(_) => {}
            }
            i += ticks;
            continue;
        }
        if inline_code_ticks.is_none()
            && i + 1 < bytes.len()
            && bytes[i] == b'['
            && bytes[i + 1] == b'['
            && let Some(close_rel) = memmem::find(&bytes[i + 2..], b"]]")
        {
            let end = i + 2 + close_rel + 2;
            let inner = &line[i + 2..i + 2 + close_rel];
            if !inner.contains('[') && !inner.contains('\n') {
                if let Some(mut link) = parse_wikilink_inner(
                    inner,
                    EdgeKind::Navigational,
                    line_abs_start + i,
                    line_abs_start + end,
                    line_no,
                ) {
                    link.raw = line[i..end].to_owned();
                    out.push(link);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
}

fn parse_wikilink_inner(
    inner: &str,
    default_kind: EdgeKind,
    byte_start: usize,
    byte_end: usize,
    line: usize,
) -> Option<WikiLink> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (before_alias, alias) = match trimmed.split_once('|') {
        Some((l, r)) => (
            l.trim(),
            Some(r.trim().to_owned()).filter(|s| !s.is_empty()),
        ),
        None => (trimmed, None),
    };

    let (before_anchor, anchor) = match before_alias.split_once('#') {
        Some((l, r)) => (l, Some(r.trim().to_owned()).filter(|s| !s.is_empty())),
        None => (before_alias, None),
    };

    let (target_and_field, repo) = match before_anchor.split_once("::") {
        Some((l, r)) => (l, Some(r.trim().to_owned()).filter(|s| !s.is_empty())),
        None => (before_anchor, None),
    };

    let (target_raw, field) = match target_and_field.rsplit_once(':') {
        Some((l, r))
            if !r.is_empty()
                && r.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.') =>
        {
            (l.trim(), Some(r.to_owned()))
        }
        _ => (target_and_field.trim(), None),
    };

    if target_raw.is_empty() {
        return None;
    }
    let kind = if default_kind == EdgeKind::Navigational && field.is_some() {
        EdgeKind::Contributing
    } else {
        default_kind
    };

    Some(WikiLink {
        raw: String::new(),
        target: target_raw.to_owned(),
        repo,
        anchor,
        field,
        alias,
        kind,
        byte_start,
        byte_end,
        line,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_matches_en_ru_parity_vector() {
        let tokens: Vec<String> = tokenize("Error->Fix, ERROR_fix! Привет-мир 2brain")
            .into_iter()
            .map(|t| t.term)
            .collect();
        assert_eq!(
            tokens,
            vec!["error", "fix", "error", "fix", "привет", "мир", "2brain"]
        );
    }

    #[test]
    fn invalid_utf8_fails_strictly_for_sem04() {
        let err = parse_document(&[0xff, 0xfe, 0x00, 0x44]).unwrap_err();
        assert_eq!(err, SyntaxError::InvalidUtf8 { valid_up_to: 0 });
    }

    #[test]
    fn parses_frontmatter_wikilinks_anchors_and_skips_code_spans() {
        let doc = "---\ntype: insight\ndescription: \"Test note\"\ntopics:\n  - \"[[memory-systems]]\"\n---\n\n# Main Title\n\nSee [[other-note|Alias]] and [[claim-1:grounds]] and [[spec::tb-mcp-sdk#^b1]].\nIgnore `[[in-inline-code]]`.\n\n```rust\n// [[in-fenced-code]]\n```\n\nParagraph with block anchor ^blk-42\n";
        let parsed = parse_document(doc.as_bytes()).unwrap();
        assert_eq!(parsed.declared_type.as_deref(), Some("insight"));
        assert_eq!(parsed.description.as_deref(), Some("Test note"));
        assert!(parsed.frontmatter_error.is_none());

        assert_eq!(parsed.wikilinks.len(), 4);
        assert_eq!(parsed.wikilinks[0].target, "memory-systems");
        assert_eq!(parsed.wikilinks[0].kind, EdgeKind::Field);
        assert_eq!(parsed.wikilinks[1].target, "other-note");
        assert_eq!(parsed.wikilinks[1].alias.as_deref(), Some("Alias"));
        assert_eq!(parsed.wikilinks[1].kind, EdgeKind::Navigational);
        assert_eq!(parsed.wikilinks[2].target, "claim-1");
        assert_eq!(parsed.wikilinks[2].field.as_deref(), Some("grounds"));
        assert_eq!(parsed.wikilinks[2].kind, EdgeKind::Contributing);
        assert_eq!(parsed.wikilinks[3].target, "spec");
        assert_eq!(parsed.wikilinks[3].repo.as_deref(), Some("tb-mcp-sdk"));
        assert_eq!(parsed.wikilinks[3].anchor.as_deref(), Some("^b1"));

        assert_eq!(parsed.anchors.len(), 2);
        assert!(matches!(
            &parsed.anchors[0],
            Anchor::Heading { level: 1, slug, .. } if slug == "main-title"
        ));
        assert!(matches!(
            &parsed.anchors[1],
            Anchor::Block { id, .. } if id == "blk-42"
        ));
    }
}
