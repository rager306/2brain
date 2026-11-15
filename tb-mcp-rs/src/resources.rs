//! Resource URIs and the member table behind them.
//!
//! A resource is addressed as `2brain://<repo>/<member-relative path>`, with the path percent-encoded so a
//! space, a `#`, or a `?` in a file name still forms a valid URI. Absolute paths would break when a workspace
//! moves, and carry no member name.
//!
//! The legacy scheme `arsumbris://` is still **accepted** on parse for one release: a client that cached a
//! URI keeps working, and the drop is a separate, versioned change (`docs/rebrand-plan.md`, tier D).

use rmcp::model::Resource;
use serde_json::Value;

/// Catalogued files per `resources/list` page.
pub(crate) const RESOURCE_PAGE: usize = 100;

/// Every member's declared root, keyed by member name.
///
/// Parsing only: the caller owns the engine read, so this module needs no transport
/// and answers the same for the same payload (see `tests/resources.rs`).
pub(crate) fn member_roots(read: &Value) -> std::collections::BTreeMap<String, String> {
    let mut roots = std::collections::BTreeMap::new();
    for member in read
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
    roots
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

/// The scheme resources are emitted with.
pub(crate) const RESOURCE_SCHEME: &str = "2brain";

/// The scheme this project used before the rebrand. Accepted on parse, never emitted.
pub(crate) const LEGACY_RESOURCE_SCHEME: &str = "arsumbris";

/// `2brain://<repo>/<path>`, with the path percent-encoded so a file name with a space,
/// a `#`, or a `?` still forms a valid URI.
pub(crate) fn resource_uri(repo: &str, path: &str) -> String {
    format!("{RESOURCE_SCHEME}://{repo}/{}", percent_encode(path))
}

/// Split a resource URI back into its member and its member-relative path.
pub(crate) fn parse_resource_uri(uri: &str) -> Option<(String, String)> {
    let rest = uri
        .strip_prefix(&format!("{RESOURCE_SCHEME}://"))
        .or_else(|| uri.strip_prefix(&format!("{LEGACY_RESOURCE_SCHEME}://")))?;
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

/// The resources `resources/list` advertises for one page of catalogued files.
///
/// Pure: the caller reads `files` and `members` from the engine and passes the payloads
/// in, so the shape of a resource URI is decided here and nowhere else.
pub(crate) fn resource_page(
    files: &Value,
    members: &std::collections::BTreeMap<String, String>,
) -> Vec<Resource> {
    let entries = files
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut resources = Vec::with_capacity(entries.len());
    for entry in &entries {
        let Some(path) = entry.get("path").and_then(Value::as_str) else {
            continue;
        };
        let repo = entry
            .get("repo")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(relative) = member_relative(repo, path, members) else {
            continue;
        };
        let name = entry
            .get("stem")
            .and_then(Value::as_str)
            .filter(|stem| !stem.is_empty())
            .unwrap_or(&relative);
        resources.push(
            Resource::new(resource_uri(repo, &relative), name)
                .with_mime_type("text/markdown")
                .with_description(format!("{repo}/{relative}")),
        );
    }
    resources
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_uri_round_trips_a_path_with_reserved_characters() {
        let uri = resource_uri("vault", "inbox/a note #1?.md");
        assert_eq!(parse_resource_uri(&uri).unwrap().0, "vault");
        assert_eq!(
            parse_resource_uri(&uri).unwrap().1,
            "inbox/a note #1?.md",
            "the encoded form must decode back to the same path: {uri}"
        );
        assert_eq!(uri, "2brain://vault/inbox/a%20note%20%231%3F.md");
    }

    #[test]
    fn the_legacy_scheme_still_parses_and_is_never_emitted() {
        // Tier D of docs/rebrand-plan.md: emit `2brain://`, accept `arsumbris://` for one release.
        assert_eq!(
            parse_resource_uri("arsumbris://vault/inbox/a.md"),
            Some(("vault".to_owned(), "inbox/a.md".to_owned()))
        );
        assert_eq!(
            parse_resource_uri("2brain://vault/inbox/a.md"),
            Some(("vault".to_owned(), "inbox/a.md".to_owned()))
        );
        assert!(resource_uri("vault", "inbox/a.md").starts_with("2brain://"), "emission is the new scheme only");
    }

    #[test]
    fn a_foreign_uri_is_not_a_resource() {
        for uri in [
            "file:///etc/passwd",
            "2brain://vault",
            "2brain:///no-member",
            "2brain://vault/",
            "2brain://vault/%zz",
        ] {
            assert!(parse_resource_uri(uri).is_none(), "{uri}");
        }
    }

    #[test]
    fn roots_come_from_a_payload_and_nowhere_else() {
        let payload = json!({"members": [
            {"repo": "vault", "root": "/root/vault"},
            {"repo": "au-engine", "root": "/root/2brain/tb-engine/"},
            {"repo": "no-root"},
        ]});
        let roots = member_roots(&payload);
        assert_eq!(roots.len(), 2, "{roots:?}");
        assert_eq!(
            member_relative("vault", "/root/vault/inbox/x.md", &roots).as_deref(),
            Some("inbox/x.md")
        );
        assert_eq!(
            member_relative("au-engine", "/root/2brain/tb-engine/src/lib.rs", &roots).as_deref(),
            Some("src/lib.rs"),
            "a trailing slash on a declared root must not double"
        );
        assert!(member_relative("unknown", "/x/y.md", &roots).is_none());
        assert!(member_relative("vault", "/elsewhere/x.md", &roots).is_none());
    }

    #[test]
    fn a_page_skips_entries_whose_member_has_no_known_root() {
        let members =
            std::collections::BTreeMap::from([("vault".to_owned(), "/root/vault".to_owned())]);
        let files = json!({"files": [
            {"path": "/root/vault/inbox/a.md", "repo": "vault", "stem": "a"},
            {"path": "/root/vault/inbox/.hidden.md", "repo": "vault"},
            {"path": "/root/other/b.md", "repo": "other", "stem": "b"},
            {"path": "/root/vault/deep/c.md", "repo": "vault", "stem": ""},
        ]});
        let page = resource_page(&files, &members);
        let uris: Vec<String> = page.iter().map(|resource| resource.uri.clone()).collect();
        assert_eq!(
            uris,
            vec![
                "2brain://vault/inbox/a.md".to_owned(),
                "2brain://vault/inbox/.hidden.md".to_owned(),
                "2brain://vault/deep/c.md".to_owned(),
            ]
        );
        assert_eq!(
            page[1].name, "inbox/.hidden.md",
            "a missing stem falls back to the member-relative path"
        );
        assert_eq!(page[2].name, "deep/c.md");
    }
}
