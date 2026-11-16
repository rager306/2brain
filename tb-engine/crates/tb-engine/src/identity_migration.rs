//! The tier-C identity migration as a pure byte splice: `.arsumbris/` becomes `.2brain/`, and
//! the engine-schema claims a repository writes are respelled `au.engine.*` / `::au-engine` ->
//! `tb.engine.*` / `::tb-engine`.
//!
//! The command that drives this is `au migrate identity` in `tb-cli`; file discovery, the
//! clean-tree / in-flight / live-daemon refusals and the per-repo commit live there. This module
//! owns the part that needs the schema: which claim the builtin owns, and what its release-2
//! spelling is. It is a SPLICE, not a re-serialization — every byte that is not a claim token is
//! carried through unchanged, so comments, key order, quoting and blank lines survive. See
//! `docs/research/tier-c-seams.md`, sections 2-4.
//!
//! A claim that can only mean the builtin's schema is respelled: a hardwired engine-schema name
//! (`au.engine.repo`, in either spelling) and the builtin repo qualifier (`::au-engine`, in
//! either spelling). Everything else is left alone, so a knowledge base's own
//! `au.engine.custom` keeps its name; only its `::au-engine` qualifier moves, which is the repo
//! identity's move, not the name's. This is the forward twin of `canonical_schema_claim`, which
//! release 1 uses to read this module's output.

use std::ops::Range;
use std::path::Path;

use crate::engine_schema::{
    NEXT_BUILTIN_ENGINE_REPO, NEXT_ENGINE_NAMESPACE, is_builtin_repo, is_hardwired_engine_name,
    schema_leaf,
};

/// One claim line the migration rewrites, as the plan reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRewrite {
    /// 1-based line number in the file.
    pub line: usize,
    /// The line as it stands, without its terminator.
    pub before: String,
    /// The same line after the splice, without its terminator.
    pub after: String,
}

/// Rewrite the engine-schema claims in `text` from the release-1 spelling to the release-2 one.
///
/// `path` decides where a claim can live: a Markdown file's leading YAML frontmatter (a fenced
/// code block in the body is prose, never a claim), every line of a YAML file. Returns the new
/// text plus the lines it changed, or `None` when there is nothing to splice — an unchanged file
/// is then never rewritten, so its bytes stay exactly as they were.
pub fn rewrite_engine_claims(path: &Path, text: &str) -> Option<(String, Vec<ClaimRewrite>)> {
    let region = claim_region(path, text)?;
    let mut out = String::with_capacity(text.len());
    let mut rewrites = Vec::new();
    let mut open_claim_block: Option<usize> = None;
    let mut offset = 0usize;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let line_start = offset;
        offset += line.len();
        let body_len = line.trim_end_matches('\n').trim_end_matches('\r').len();
        let (body, terminator) = line.split_at(body_len);
        let spliced = if line_start >= region.start && line_start < region.end {
            splice_line(body, &mut open_claim_block)
        } else {
            open_claim_block = None;
            None
        };
        match spliced {
            Some(after) => {
                rewrites.push(ClaimRewrite {
                    line: index + 1,
                    before: body.to_string(),
                    after: after.clone(),
                });
                out.push_str(&after);
            }
            None => out.push_str(body),
        }
        out.push_str(terminator);
    }
    (!rewrites.is_empty()).then_some((out, rewrites))
}

/// Every `type:` / `extends:` line of one question, but only the ones the builtin already owns:
/// a claim that identifies a knowledge base's own type is left as the author wrote it.
fn next_claim_spelling(claim: &str) -> Option<String> {
    let (name, repo) = match claim.split_once("::") {
        Some((name, repo)) => (name, Some(repo)),
        None => (claim, None),
    };
    let next_name = next_name_spelling(name);
    let next_repo = repo.and_then(next_repo_spelling);
    if next_name.is_none() && next_repo.is_none() {
        return None;
    }
    let mut out = next_name.unwrap_or_else(|| name.to_string());
    if let Some(repo) = repo {
        out.push_str("::");
        out.push_str(next_repo.as_deref().unwrap_or(repo));
    }
    Some(out)
}

/// The release-2 spelling of a hardwired engine-schema name, `None` when the name is not the
/// builtin's or already spells the new namespace.
fn next_name_spelling(name: &str) -> Option<String> {
    if !is_hardwired_engine_name(name) {
        return None;
    }
    let leaf = schema_leaf(name)?;
    let next = format!("{NEXT_ENGINE_NAMESPACE}{leaf}");
    (next != name).then_some(next)
}

/// The release-2 spelling of a repo qualifier, `None` when it does not name the builtin or
/// already spells the new name.
fn next_repo_spelling(repo: &str) -> Option<String> {
    is_builtin_repo(repo)
        .then(|| NEXT_BUILTIN_ENGINE_REPO.to_string())
        .filter(|next| next != repo)
}

/// The byte range of the claim-bearing region: a Markdown file's leading YAML frontmatter, the
/// whole text of a YAML file. `None` for Markdown with no frontmatter, which can carry no claim.
fn claim_region(path: &Path, text: &str) -> Option<Range<usize>> {
    if !is_markdown(path) {
        return Some(0..text.len());
    }
    let mut offset = 0usize;
    let mut opened: Option<usize> = None;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let body = line.trim_end_matches('\n').trim_end_matches('\r');
        if body.trim() != "---" {
            if opened.is_none() {
                return None;
            }
            continue;
        }
        match opened {
            None => opened = Some(offset),
            Some(start) => return Some(start..line_start),
        }
    }
    None
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("md") || extension.eq_ignore_ascii_case("markdown")
        })
}

/// Splice one line. `open_claim_block` carries an open block the previous line started
/// (`extends:` with nothing after the colon), so an indented `- claim` item under it is spliced
/// too. A blank or comment line neither closes nor opens a block, as in YAML.
fn splice_line(body: &str, open_claim_block: &mut Option<usize>) -> Option<String> {
    let indent = body.len() - body.trim_start().len();
    let trimmed = body.trim_start();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix('-') {
        let open = open_claim_block.is_some_and(|key_indent| indent > key_indent);
        if !open {
            *open_claim_block = None;
            return None;
        }
        let lead = rest.len() - rest.trim_start().len();
        let (claim, tail) = split_claim(&rest[lead..]);
        let next = next_claim_spelling(claim)?;
        return Some(format!(
            "{}-{}{}{}",
            &body[..indent],
            &rest[..lead],
            next,
            tail
        ));
    }
    *open_claim_block = None;
    let (key, rest) = trimmed.split_once(':')?;
    if !is_claim_key(key.trim_end()) {
        return None;
    }
    let lead = rest.len() - rest.trim_start().len();
    let value = &rest[lead..];
    if value.is_empty() {
        *open_claim_block = Some(indent);
        return None;
    }
    let (claim, tail) = split_claim(value);
    let next = next_claim_spelling(claim)?;
    Some(format!(
        "{}{}:{}{}{}",
        &body[..indent],
        key,
        &rest[..lead],
        next,
        tail
    ))
}

/// The two claim keys an engine-schema file writes. A `type:` inside a `fields:` block names a
/// field, and its value is a type expression, never a claim token, so it is left alone by
/// construction: no engine-schema name appears as a field's type.
fn is_claim_key(key: &str) -> bool {
    key == "type" || key == "extends"
}

/// A claim token is the first whitespace-delimited word of the value; the rest is a trailing
/// comment and is carried through untouched.
fn split_claim(value: &str) -> (&str, &str) {
    let end = value
        .find(|c: char| c.is_whitespace())
        .unwrap_or(value.len());
    value.split_at(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Option<(String, Vec<ClaimRewrite>)> {
        rewrite_engine_claims(Path::new("repo.yaml"), text)
    }

    fn markdown(text: &str) -> Option<(String, Vec<ClaimRewrite>)> {
        rewrite_engine_claims(Path::new("README.md"), text)
    }

    #[test]
    fn a_legacy_identity_claim_is_respelled_in_place() {
        let (out, lines) =
            yaml("#: the repo's identity\ntype: au.engine.repo::au-engine\nname: demo\n")
                .expect("a legacy claim is a rewrite");
        assert_eq!(
            out,
            "#: the repo's identity\ntype: tb.engine.repo::tb-engine\nname: demo\n"
        );
        assert_eq!(
            lines,
            vec![ClaimRewrite {
                line: 2,
                before: "type: au.engine.repo::au-engine".to_string(),
                after: "type: tb.engine.repo::tb-engine".to_string(),
            }]
        );
    }

    #[test]
    fn every_engine_schema_file_kind_moves() {
        for (before, after) in [
            (
                "type: au.engine.repo::au-engine",
                "type: tb.engine.repo::tb-engine",
            ),
            (
                "type: au.engine.workspace::au-engine",
                "type: tb.engine.workspace::tb-engine",
            ),
            (
                "type: au.engine.workspace-lock::au-engine",
                "type: tb.engine.workspace-lock::tb-engine",
            ),
            (
                "type: au.engine.readme::au-engine",
                "type: tb.engine.readme::tb-engine",
            ),
            (
                "extends: au.engine.meta::au-engine",
                "extends: tb.engine.meta::tb-engine",
            ),
        ] {
            let (out, _) = yaml(&format!("{before}\n")).expect("rewrites");
            assert_eq!(out, format!("{after}\n"));
        }
    }

    #[test]
    fn a_knowledge_base_engine_named_type_keeps_its_name() {
        // The namespace is reserved, not owned: a KB def may be named `au.engine.custom` and
        // keeps that identity. Only the builtin repo qualifier moves.
        let (out, _) = yaml("type: au.engine.custom::au-engine\n").expect("the qualifier moves");
        assert_eq!(out, "type: au.engine.custom::tb-engine\n");
        assert_eq!(
            yaml("type: au.engine.custom\n"),
            None,
            "an unqualified KB name is untouched"
        );
        // The qualifier is the builtin REPO identity, so it moves even under a name the builtin
        // does not own: `other::au-engine` names a type in the engine's own repo.
        let (out, _) = yaml("type: other::au-engine\n").expect("the qualifier moves");
        assert_eq!(out, "type: other::tb-engine\n");
    }

    #[test]
    fn indentation_a_trailing_comment_and_a_block_list_survive() {
        let text = "\
extends:
  - theme::base   # a peer, kept
  - au.engine.meta::au-engine   # the engine meta base
#: a comment after the block
fields:
  name: String
";
        let (out, lines) = yaml(text).expect("one line rewrites");
        assert_eq!(
            out,
            "\
extends:
  - theme::base   # a peer, kept
  - tb.engine.meta::tb-engine   # the engine meta base
#: a comment after the block
fields:
  name: String
"
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].line, 3);
    }

    #[test]
    fn a_markdown_body_is_prose_and_its_frontmatter_is_a_claim() {
        let text = "\
---
type: au.engine.readme::au-engine
tldr: demo
---

# Demo

```yaml
type: au.engine.repo::au-engine
```
";
        let (out, lines) = markdown(text).expect("the frontmatter rewrites");
        assert_eq!(
            out,
            "\
---
type: tb.engine.readme::tb-engine
tldr: demo
---

# Demo

```yaml
type: au.engine.repo::au-engine
```
"
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].line, 2);
    }

    #[test]
    fn markdown_without_frontmatter_has_no_claims() {
        assert_eq!(
            markdown("# Demo\n\ntype: au.engine.repo::au-engine\n"),
            None
        );
        assert_eq!(
            markdown("\n---\ntype: au.engine.repo::au-engine\n---\n"),
            None
        );
    }

    #[test]
    fn the_new_spelling_is_already_migrated() {
        assert_eq!(yaml("type: tb.engine.repo::tb-engine\n"), None);
        assert_eq!(yaml("name: demo\n"), None);
        assert_eq!(
            yaml("extends:\n  - tb.engine.meta::tb-engine\n"),
            None,
            "a block list in the new spelling is untouched"
        );
        // A half-migrated claim still moves: each half is judged on its own.
        let (out, _) = yaml("type: au.engine.repo::tb-engine\n").expect("the name half moves");
        assert_eq!(out, "type: tb.engine.repo::tb-engine\n");
    }

    #[test]
    fn a_second_pass_is_a_no_op() {
        let text = "type: au.engine.repo::au-engine\n";
        let (once, _) = yaml(text).expect("first pass rewrites");
        assert_eq!(rewrite_engine_claims(Path::new("repo.yaml"), &once), None);
    }

    #[test]
    fn a_crlf_line_keeps_its_terminator() {
        let text = "type: au.engine.repo::au-engine\r\nname: demo\r\n";
        let (out, lines) = yaml(text).expect("rewrites");
        assert_eq!(out, "type: tb.engine.repo::tb-engine\r\nname: demo\r\n");
        assert_eq!(lines[0].before, "type: au.engine.repo::au-engine");
    }

    #[test]
    fn a_flow_sequence_is_not_a_scalar_claim() {
        // `type: [a, b]` has no single token to splice; the migration leaves it for a human.
        assert_eq!(yaml("type: [au.engine.repo::au-engine]\n"), None);
    }
}
