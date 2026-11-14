//! Resource URIs and the member table behind them.
//!
//! A resource is addressed as `arsumbris://<repo>/<member-relative path>`, with
//! the path percent-encoded so a space, a `#`, or a `?` in a file name still
//! forms a valid URI. Absolute paths would break when a workspace moves, and
//! carry no member name.

use std::path::Path;

use serde_json::{Value, json};

use crate::wire::read_engine;

/// Catalogued files per `resources/list` page.
pub(crate) const RESOURCE_PAGE: usize = 100;

/// Every member's declared root, keyed by member name.
pub(crate) async fn member_roots(
    socket: &Path,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let read = read_engine(socket, json!({"read": "members", "id": 1}), "members").await?;
    let mut roots = std::collections::BTreeMap::new();
    for member in read
        .payload
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(repo), Some(root)) = (
            member.get("repo").and_then(Value::as_str),
            member.get("root").and_then(Value::as_str),
        ) {
            roots.insert(repo.to_owned(), root.to_owned());
        }
    }
    Ok(roots)
}

/// The member-relative form of a catalogued absolute path, or None when the
/// member's root is unknown or does not contain it.
pub(crate) fn member_relative(
    repo: &str,
    path: &str,
    roots: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    let root = roots.get(repo)?;
    let prefix = format!("{}/", root.trim_end_matches('/'));
    path.strip_prefix(&prefix).map(str::to_owned)
}

/// `arsumbris://<repo>/<path>`, with the path percent-encoded so a file name with
/// a space, a `#`, or a `?` still forms a valid URI.
pub(crate) fn resource_uri(repo: &str, path: &str) -> String {
    format!("arsumbris://{repo}/{}", percent_encode(path))
}

/// Split a resource URI back into its member and its member-relative path.
pub(crate) fn parse_resource_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri.strip_prefix("arsumbris://")?;
    let (repo, path) = rest.split_once('/')?;
    if repo.is_empty() || path.is_empty() {
        return None;
    }
    Some((repo.to_owned(), percent_decode(path)?))
}

/// Percent-encode everything outside the RFC 3986 unreserved set plus `/`.
pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Decode `%XX` escapes; anything malformed is a bad URI, not a wrong file.
pub(crate) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = value.get(index + 1..index + 3)?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                index += 3;
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}
