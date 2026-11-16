//! The source-identity lookup behind a URL query.
//!
//! A query that is itself a URL is a lookup, not a ranking: the note whose
//! `source_url:` frontmatter names that address is the answer, and no amount of
//! BM25 over prose can say so, because the index holds body and description and
//! never the URL field (`docs/research/question-set-baseline-2026-11-16.md`,
//! row `z08`: a pasted `https://arxiv.org/abs/...` matched 109 documents that
//! talk around the address and missed the 16 notes that carry it).
//!
//! The identity a URL normalizes to is the one the provenance pass already
//! groups notes by (`docs/research/lineage-echoes-2026-11-16.md`): lower-case
//! host, no `www.`, no scheme, fragment, or query string, no trailing slash,
//! and three known equivalences. Collapsing on BOTH sides — the query and the
//! note's own value — is what makes `.../abs/<id>` and `.../pdf/<id>` find the
//! same notes.
//!
//! The scan walks the catalogue once per URL-shaped query and compares
//! normalized strings, so it is bounded by the catalogue the index was built
//! from and reads no disk.

use std::path::PathBuf;

use tb_core::{InstanceField, InstanceValue};

use crate::ir::{FileEntry, OrdMap};
use crate::parse::FileParse;

/// A URL's source identity: the key two URLs must agree on to name one source.
///
/// The provenance rules, in order: trim, drop the scheme, cut the fragment,
/// cut the query string (its parameters are tracking, not identity), lower-case
/// the authority and drop one leading `www.`, strip trailing slashes, then
/// collapse the three known equivalences:
///
/// - `arxiv.org/abs|pdf|html/<id>` collapses to `arxiv:<id>`,
/// - `docs.google.com/document/d/<id>` collapses to `gdoc:<id>`,
/// - `github.com/<owner>/<repo>` collapses to `gh:<owner>/<repo>`, case-folded,
///
/// and anything else stands as `host/path`. `None` when the text is not an
/// http(s) URL or carries no host: such a string names no source to find.
pub(crate) fn identity(url: &str) -> Option<String> {
    let rest = strip_scheme(url.trim())?;
    // Fragment and query string end the identity wherever they start; the
    // earlier cut wins, and both may be absent.
    let cut = rest.find(['#', '?']).unwrap_or(rest.len());
    let (authority, path) = match rest[..cut].find('/') {
        Some(slash) => (&rest[..slash], &rest[slash + 1..cut]),
        None => (&rest[..cut], ""),
    };
    if authority.is_empty() {
        return None;
    }
    let lowered = authority.to_ascii_lowercase();
    let host = lowered.strip_prefix("www.").unwrap_or(lowered.as_str());
    let path = path.trim_end_matches('/');
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    match (host, segments.as_slice()) {
        ("arxiv.org", [kind, id]) if matches!(*kind, "abs" | "pdf" | "html") => {
            Some(format!("arxiv:{id}"))
        }
        ("docs.google.com", ["document", "d", id, ..]) => Some(format!("gdoc:{id}")),
        ("github.com", [owner, repo, ..]) => Some(format!(
            "gh:{}/{}",
            owner.to_lowercase(),
            repo.to_lowercase()
        )),
        _ => Some(match path.is_empty() {
            true => host.to_string(),
            false => format!("{host}/{path}"),
        }),
    }
}

/// Drop an `http://` / `https://` scheme, case-insensitively; `None` otherwise.
fn strip_scheme(url: &str) -> Option<&str> {
    for scheme in ["http://", "https://"] {
        if lowered_prefix_is(url, scheme) {
            return Some(&url[scheme.len()..]);
        }
    }
    None
}

/// Whether `text` begins with `prefix`, compared byte-wise so a query that
/// opens with a multi-byte character cannot slice mid-character.
fn lowered_prefix_is(text: &str, prefix: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() >= prefix.len() && bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// A note's `source_url:` frontmatter value, `None` when absent or not a
/// string.
///
/// Both shapes that carry top-level frontmatter: a typed instance and a
/// frontmatter note. A type-def or an unparsed file names no source.
pub(crate) fn frontmatter_source_url(parse: &FileParse) -> Option<&str> {
    let fields = match parse {
        FileParse::Instance { instance, .. } => instance.as_ref()?.fields.as_slice(),
        FileParse::Note { fields, .. } => fields.as_slice(),
        _ => return None,
    };
    string_field(fields, "source_url")
}

/// The value of the named top-level string field, `None` when the key is
/// absent or its value is not a string.
fn string_field<'a>(fields: &'a [InstanceField], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|field| match (&*field.key, &field.value) {
            (k, InstanceValue::String(text)) if k == key => Some(text.as_str()),
            _ => None,
        })
}

/// One catalogue note whose `source_url` names the queried source.
pub(crate) struct SourceHit {
    /// The note's catalogue path.
    pub path: PathBuf,
    /// The `source_url:` as the note authored it, echoed so a caller can
    /// compare what it pasted against what the note declares.
    pub note_url: String,
    /// The identity both URLs normalize to.
    pub identity: String,
}

/// Every catalogued note whose `source_url` normalizes to the query's
/// identity, in catalogue path order — the same order the ranking's document
/// ids and tie rule use.
///
/// An empty answer is the ordinary case twice over: the query was words, not a
/// URL, or it was a URL this base holds no `source_url` for. Both fall through
/// to the ranking unchanged.
pub(crate) fn lookup(catalog: &OrdMap<PathBuf, FileEntry>, query: &str) -> Vec<SourceHit> {
    let Some(want) = identity(query) else {
        return Vec::new();
    };
    let mut hits = Vec::new();
    for (path, entry) in catalog {
        let Some(url) = frontmatter_source_url(&entry.parse) else {
            continue;
        };
        if identity(url).as_deref() == Some(want.as_str()) {
            hits.push(SourceHit {
                path: path.clone(),
                note_url: url.to_string(),
                identity: want.clone(),
            });
        }
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scheme_is_dropped_and_the_host_is_lower_cased() {
        assert_eq!(
            identity("HTTPS://EXAMPLE.COM/Page"),
            Some("example.com/Page".to_string())
        );
        assert_eq!(
            identity("http://example.com"),
            Some("example.com".to_string())
        );
        // A non-http scheme names no lookup-able source, so a query in that
        // shape is never mistaken for a source lookup.
        assert_eq!(identity("ftp://example.com/a"), None);
        assert_eq!(identity("example.com/a"), None);
        assert_eq!(identity("https:/not-a-url"), None);
        assert_eq!(identity(""), None);
    }

    #[test]
    fn a_leading_www_is_dropped() {
        assert_eq!(
            identity("https://www.example.com/a"),
            identity("https://example.com/a")
        );
        // One leading label only: a host that merely contains `www` stands.
        assert_eq!(
            identity("https://awww.example.com/a"),
            Some("awww.example.com/a".to_string())
        );
    }

    #[test]
    fn the_fragment_is_dropped() {
        assert_eq!(
            identity("https://example.com/a#section"),
            identity("https://example.com/a")
        );
    }

    #[test]
    fn the_query_string_is_dropped() {
        assert_eq!(
            identity("https://example.com/a?utm_source=x&id=7"),
            identity("https://example.com/a")
        );
        // The earlier of fragment and query string ends the identity.
        assert_eq!(
            identity("https://example.com/a?x=1#frag"),
            identity("https://example.com/a")
        );
        assert_eq!(
            identity("https://example.com/a#frag?x=1"),
            identity("https://example.com/a")
        );
    }

    #[test]
    fn a_trailing_slash_is_dropped() {
        assert_eq!(
            identity("https://example.com/a/"),
            identity("https://example.com/a")
        );
        assert_eq!(
            identity("https://example.com/"),
            identity("https://example.com")
        );
    }

    #[test]
    fn an_arxiv_abs_pdf_html_id_collapses_to_one_identity() {
        let abs = identity("https://arxiv.org/abs/2602.20021").expect("an arxiv identity");
        assert_eq!(abs, "arxiv:2602.20021");
        assert_eq!(
            identity("https://arxiv.org/pdf/2602.20021"),
            Some(abs.clone())
        );
        assert_eq!(
            identity("https://arxiv.org/html/2602.20021"),
            Some(abs.clone())
        );
        // The equivalence is host-scoped: a lookalike path elsewhere stands.
        assert_eq!(
            identity("https://example.org/abs/2602.20021"),
            Some("example.org/abs/2602.20021".to_string())
        );
        // Without the id there is nothing to collapse.
        assert_eq!(
            identity("https://arxiv.org/abs"),
            Some("arxiv.org/abs".to_string())
        );
    }

    #[test]
    fn a_google_doc_collapses_to_its_file_id() {
        let one = identity("https://docs.google.com/document/d/FILE123/edit");
        assert_eq!(one, Some("gdoc:FILE123".to_string()));
        assert_eq!(identity("https://docs.google.com/document/d/FILE123"), one);
        // Not a document URL: the plain path identity stands.
        assert_eq!(
            identity("https://docs.google.com/spreadsheets/d/FILE123"),
            Some("docs.google.com/spreadsheets/d/FILE123".to_string())
        );
    }

    #[test]
    fn a_github_repo_collapses_case_folded_and_ignores_deeper_paths() {
        let repo = identity("https://github.com/Owner/Repo");
        assert_eq!(repo, Some("gh:owner/repo".to_string()));
        assert_eq!(identity("https://github.com/owner/repo/tree/main"), repo);
        assert_eq!(identity("https://www.github.com/OWNER/REPO"), repo);
        // One segment names a user, not a repository: no equivalence.
        assert_eq!(
            identity("https://github.com/owner"),
            Some("github.com/owner".to_string())
        );
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            identity("  https://example.com/a  "),
            Some("example.com/a".to_string())
        );
    }

    #[test]
    fn a_note_url_and_a_pasted_query_meet_on_the_same_identity() {
        let note = "https://arxiv.org/pdf/2602.20021v2";
        let query = "http://www.arxiv.org/abs/2602.20021v2#conclusion";
        assert_eq!(identity(note), identity(query));
    }
}
