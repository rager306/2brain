//! The reference rewrite for a file rename: repoint every inbound wikilink at
//! the new name, keeping the mounted-set graph consistent.
//!
//! The backlink index locates each reference (which file, which byte span); the
//! source on disk is the truth for the exact text. So the rewrite re-reads each
//! referrer at its span, re-parses the wikilink, and re-serializes it with the
//! new target — preserving the repo qualifier and every fragment (anchor,
//! block-id, field).
//!
//! The new target preserves the referrer's spelling mode. A file resolves
//! several ways (mirroring [`au_references`] / [`crate::refnames`]): a bare stem
//! or basename, a `/`-bearing repo-relative path, or a `/`-bearing
//! extensionless dir-and-stem. The original target is matched against the old
//! path's spellings to recover its mode, then the new path is rendered in the
//! same mode. `[[old]]` stays bare, `[[notes/old]]` stays a path, `[[old.md]]`
//! keeps its extension.
//!
//! A commit-pinned reference is exempt. It names a target as of a commit, so
//! re-pointing it would falsify a historical record; [`ref_edits`] freezes it
//! and every other verb inherits that, since they all route through there.

use std::path::Path;

use au_diagnostics::ByteRange;
use au_references::{parse_wikilink, WikilinkRef};

use crate::mutate::MutationReject;

/// Serialize a wikilink from its parts, in canonical order:
/// `[[target::repo@commit#anchor^block_id:field]]`. The target and every
/// fragment come from `w`, so a caller rewrites by handing in a transformed
/// [`WikilinkRef`]. A `@commit` pin binds to `::`, so the `::` is emitted
/// whenever a repo or a commit is present — `::@commit` is the this-repo pin.
///
/// A pinned reference never reaches here through a refactor: [`ref_edits`]
/// freezes it upstream. The `@commit` arm stays because this is the general
/// serializer, and a future caller constructing a pin needs it to round-trip.
pub(crate) fn serialize_wikilink(w: &WikilinkRef) -> String {
    let mut s = String::from("[[");
    s.push_str(&w.target);
    if w.repo.is_some() || w.commit.is_some() {
        s.push_str("::");
        if let Some(r) = &w.repo {
            s.push_str(r);
        }
    }
    if let Some(c) = &w.commit {
        s.push('@');
        s.push_str(c);
    }
    if let Some(a) = &w.anchor {
        s.push('#');
        s.push_str(a);
    }
    if let Some(b) = &w.block_id {
        // A block-referent `^^id` re-serializes with the doubled caret; a bare
        // `^id` stays navigational. Dropping the doubling would silently
        // downgrade a value-reference to an anchor across every refactor.
        s.push('^');
        if b.referent {
            s.push('^');
        }
        s.push_str(&b.id);
    }
    if let Some(f) = &w.field {
        s.push(':');
        s.push_str(f);
    }
    s.push_str("]]");
    s
}

/// The `/`-bearing extensionless spelling of a repo-relative path: `parent/stem`.
/// `None` for a top-level file, where it is not distinct from the bare stem.
fn dir_stem(rel: &Path) -> Option<String> {
    let stem = rel.file_stem()?.to_string_lossy().into_owned();
    match rel.parent() {
        Some(p) if !p.as_os_str().is_empty() => Some(format!("{}/{stem}", p.to_string_lossy())),
        _ => None,
    }
}

/// The new target for one reference, preserving the spelling mode the original
/// used. The original is matched, case-insensitively, against the old path's
/// spellings — exact relpath, dir-and-stem, basename, stem — and the new path is
/// rendered in the matched mode. An unrecognized spelling defaults to the bare
/// stem, which always resolves for a uniquely-named file.
fn rewritten_target(original: &str, old_rel: &Path, new_rel: &Path) -> String {
    let orig = original.trim().to_ascii_lowercase();
    let ci = |s: &str| s.to_ascii_lowercase();

    let new_relpath = new_rel.to_string_lossy().into_owned();
    let new_stem = new_rel
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let new_basename = new_rel
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Most specific first: full relpath, then dir-stem, then basename, then the
    // type-name (a def file's `.type`-stripped name), then the bare stem.
    //
    // The resolver decides a link's mode by its text alone, a `/`-bearing target
    // is a path, anything else a name ([[reference name resolution::au-type-system]]).
    // So the relpath mode needs the `/`: a top-level file's relpath equals its
    // basename, and `[[a.md]]` there is a basename link, re-rendered as a basename.
    if orig.contains('/') && orig == ci(&old_rel.to_string_lossy()) {
        new_relpath
    } else if dir_stem(old_rel).is_some_and(|d| orig == ci(&d)) {
        dir_stem(new_rel).unwrap_or(new_stem)
    } else if old_rel
        .file_name()
        .is_some_and(|b| orig == ci(&b.to_string_lossy()))
    {
        new_basename
    } else if au_core::type_name_from_path(old_rel).is_some_and(|t| orig == ci(t.as_str())) {
        // A type-def file is reachable by its type-name, the `.type` tail
        // stripped ([[type reference::au-type-system]] Name Resolution). A `[[mcp.tool]]`
        // def-ref value spells the type-name, neither stem nor basename; without
        // this branch it would normalise to the new stem (`mcp.device.type`).
        // Inert for non-type files, where `type_name_from_path` is `None`; and
        // for a `.yaml` instance, where the type-name equals the stem, so it
        // agrees with the default below.
        au_core::type_name_from_path(new_rel)
            .map(|t| t.as_str().to_string())
            .unwrap_or(new_stem)
    } else {
        // The bare stem, and the default for any unrecognized spelling.
        new_stem
    }
}

/// Rewrite every reference at `spans` in `content`, each re-pointed by `transform`.
///
/// The index locates a `[[...]]` by byte span; the source on disk is the truth
/// for its text, so each span is re-sliced and re-parsed, then `transform` maps
/// the parsed [`WikilinkRef`] to its replacement — a new target, a changed or
/// dropped fragment, whatever the refactor demands. Rename preserves the form
/// and swaps the target; promote and inline change the form (block-ref to
/// file-ref and back). Spans are non-overlapping (distinct wikilinks) and applied
/// last-first so an earlier edit never shifts a later span.
pub(crate) fn rewrite_refs(
    content: &str,
    spans: &[ByteRange],
    transform: impl FnMut(&WikilinkRef) -> WikilinkRef,
) -> Result<String, MutationReject> {
    apply_edits(content, ref_edits(content, spans, transform)?)
}

/// Compute the (span, replacement) edits for every reference at `spans`, each
/// re-pointed by `transform`. The edits are not yet applied — a caller can merge
/// them with other edits on the same file (e.g. `promote`'s record replacement)
/// and apply the union in one [`apply_edits`] pass.
///
/// A COMMIT-PINNED reference is frozen: it yields no edit, so its bytes survive
/// the refactor untouched. A pin `[[old::@sha]]` asserts the target was called
/// `old` at `sha`; re-pointing it at the new name would edit a historical record
/// into saying something that never happened. The freeze covers the WHOLE
/// reference, fragments included — a `^block-id` on a pin carries the same claim
/// about that commit as the name does.
///
/// This is the one choke point every reference-rewriting verb passes through, so
/// the freeze holds for `rename`, `promote`, `inline`, `rename_block_id`, and
/// `rename_type` alike. `typerefs::type_ref_edits` rewrites type-name CLAIMS
/// rather than wikilinks, so it carries no pin and needs no counterpart.
///
/// The consequence, stated because a consumer sees it: a commit-pinned edge is
/// inert, it forms no backlink at all, so a rename has nothing to re-point and an
/// inbound query never lists it. The record stays true, and reconstructing where
/// the pinned content went now is a consumer's lineage tool, not the pin's. See
/// [[spec - pinned references - a recorded resolved edge with an immutable past and an on-demand forward trace]].
pub(crate) fn ref_edits(
    content: &str,
    spans: &[ByteRange],
    mut transform: impl FnMut(&WikilinkRef) -> WikilinkRef,
) -> Result<Vec<(ByteRange, String)>, MutationReject> {
    let mut edits: Vec<(ByteRange, String)> = Vec::with_capacity(spans.len());
    for span in spans {
        let slice = content.get(span.start..span.end).ok_or_else(|| {
            MutationReject::new(
                "a reference span is out of range — the file drifted from the index",
            )
        })?;
        if frozen_span(content, *span) {
            continue;
        }
        let w = parse_wikilink(slice).map_err(|_| {
            MutationReject::new(format!("could not re-parse the reference {slice:?}"))
        })?;
        edits.push((*span, serialize_wikilink(&transform(&w))));
    }
    Ok(edits)
}

/// Whether the reference at `span` is FROZEN, exempt from rewriting because it
/// is commit-pinned.
///
/// The single place the freeze is decided. [`ref_edits`] skips these, and
/// [`would_rewrite`] answers from the same test, so the saga planner and the
/// rewriter cannot drift into disagreeing about which files a refactor writes.
///
/// An unparseable or out-of-range span is NOT frozen. `ref_edits` rejects the
/// whole mutation on one, so answering "not frozen" keeps the referrer in the
/// writable set and lets that rejection happen where it can carry a message,
/// rather than being silently dropped from the mutation here.
fn frozen_span(content: &str, span: ByteRange) -> bool {
    content
        .get(span.start..span.end)
        .and_then(|slice| parse_wikilink(slice).ok())
        .is_some_and(|w| w.commit.is_some())
}

/// Whether rewriting this referrer would change it, i.e. whether [`ref_edits`]
/// would produce any edit for `spans`.
///
/// A referrer whose every reference is frozen is NOT written by the refactor, so
/// it must not be subject to the clean-at-HEAD guard: that guard exists because
/// the engine is the writer, and it cannot clobber uncommitted work in a file it
/// never touches. It still needs RECOMPUTING, since a frozen pin stops resolving
/// live once its target moves, so the two sets are deliberately different.
pub(crate) fn would_rewrite(content: &str, spans: &[ByteRange]) -> bool {
    spans.iter().any(|span| !frozen_span(content, *span))
}

/// Apply non-overlapping `edits` to `content`, last span first so an earlier
/// edit never shifts a later span's offsets. Each span is bounds-checked against
/// `content`; an out-of-range span rejects rather than panicking the slice.
pub(crate) fn apply_edits(
    content: &str,
    mut edits: Vec<(ByteRange, String)>,
) -> Result<String, MutationReject> {
    for (span, _) in &edits {
        if span.end > content.len()
            || !content.is_char_boundary(span.start)
            || !content.is_char_boundary(span.end)
        {
            return Err(MutationReject::new(
                "an edit span is out of range — the file drifted from the index",
            ));
        }
    }
    edits.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    // Reject overlapping spans. The edits apply last-span-first, so an overlap
    // would silently corrupt rather than fail. A single-source caller
    // (`rewrite_refs`) never overlaps — distinct wikilinks. `rename_type` merges
    // two independently-computed edit sets (type-name + wikilink), disjoint
    // today only as an emergent property of two walkers, so the invariant is
    // enforced here, not trusted. Sorted descending by start, so a pair overlaps
    // when the lower-start span's end reaches past the higher-start span's start.
    for pair in edits.windows(2) {
        let (hi, _) = &pair[0];
        let (lo, _) = &pair[1];
        if lo.end > hi.start {
            return Err(MutationReject::new(
                "overlapping edit spans — a reference rewrite would corrupt the file",
            ));
        }
    }
    let mut out = content.to_string();
    for (span, text) in &edits {
        out.replace_range(span.start..span.end, text);
    }
    Ok(out)
}

/// The (span, replacement) edits a file move's reference rewrite produces, the
/// mergeable form of a file move's rewrite. A caller can union these with other
/// edits on the same file — e.g. `rename_type`'s type-name-claim edits — and
/// apply the whole set in one [`apply_edits`] pass.
pub(crate) fn link_edits(
    content: &str,
    spans: &[ByteRange],
    old_rel: &Path,
    new_rel: &Path,
) -> Result<Vec<(ByteRange, String)>, MutationReject> {
    ref_edits(content, spans, |w| {
        let mut nw = w.clone();
        nw.target = rewritten_target(&w.target, old_rel, new_rel);
        nw
    })
}

/// One moved file as a referrer sees it: the spans of the referrer's references
/// to it, and its repo-relative paths before and after the move.
pub(crate) struct MovedTarget<'a> {
    pub spans: &'a [ByteRange],
    pub old_rel: &'a Path,
    pub new_rel: &'a Path,
}

/// The edits a set of file moves makes to one referrer, the union of each moved
/// file's [`link_edits`]. A referrer's links resolve to distinct files, so the
/// per-target span sets never overlap, and [`apply_edits`] enforces it.
fn moves_link_edits(
    content: &str,
    targets: &[MovedTarget<'_>],
) -> Result<Vec<(ByteRange, String)>, MutationReject> {
    let mut edits = Vec::new();
    for t in targets {
        edits.extend(link_edits(content, t.spans, t.old_rel, t.new_rel)?);
    }
    Ok(edits)
}

/// What a set of file moves does to one referrer: `Some(new content)` when the
/// rewrite changes its bytes, `None` when it leaves them identical. Every moved
/// file the referrer points at is re-pointed in one content pass, one write.
///
/// A rewrite re-renders each reference in the referrer's own spelling mode, so a
/// move that keeps the file's name leaves a bare `[[name]]` byte-identical, only
/// a path-addressed `[[dir/name]]` changes. A commit-pinned reference is frozen,
/// see [`ref_edits`]. `None` is therefore "this referrer is not written".
///
/// The one predicate the committing move, its preview, and the saga planner's
/// not-written set all answer from, for a single-file `rename` and a folder
/// `move_dir` alike, so none of them can disagree about which referrers a move
/// writes.
pub(crate) fn moves_rewrite(
    content: &str,
    targets: &[MovedTarget<'_>],
) -> Result<Option<String>, MutationReject> {
    let rewritten = apply_edits(content, moves_link_edits(content, targets)?)?;
    Ok((rewritten != content).then_some(rewritten))
}

/// [`moves_rewrite`] for one moved file.
pub(crate) fn move_rewrite(
    content: &str,
    spans: &[ByteRange],
    old_rel: &Path,
    new_rel: &Path,
) -> Result<Option<String>, MutationReject> {
    moves_rewrite(
        content,
        &[MovedTarget {
            spans,
            old_rel,
            new_rel,
        }],
    )
}

/// The links a set of file moves re-spells in one referrer, each as
/// `(before, after)`, in document order. A link the move re-renders identically,
/// or a frozen pin, is absent. Answers from the same edits [`moves_rewrite`]
/// applies, so a referrer lists a change exactly when `moves_rewrite` reports one.
pub(crate) fn moves_link_changes(
    content: &str,
    targets: &[MovedTarget<'_>],
) -> Result<Vec<(String, String)>, MutationReject> {
    let mut edits = moves_link_edits(content, targets)?;
    edits.sort_by_key(|(span, _)| span.start);
    Ok(edits
        .into_iter()
        .filter_map(|(span, text)| {
            let before = &content[span.start..span.end];
            (before != text).then(|| (before.to_string(), text))
        })
        .collect())
}

/// [`moves_link_changes`] for one moved file.
pub(crate) fn move_link_changes(
    content: &str,
    spans: &[ByteRange],
    old_rel: &Path,
    new_rel: &Path,
) -> Result<Vec<(String, String)>, MutationReject> {
    moves_link_changes(
        content,
        &[MovedTarget {
            spans,
            old_rel,
            new_rel,
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn link_changes_list_only_the_re_spelled_links_in_source_order() {
        let content = "[[x/a]] then [[a]] then [[x/a.md]]\n";
        let spans: Vec<ByteRange> = ["[[x/a]]", "[[a]]", "[[x/a.md]]"]
            .iter()
            .map(|n| span_of(content, n))
            .collect();
        let changes = move_link_changes(
            content,
            &spans,
            &PathBuf::from("x/a.md"),
            &PathBuf::from("y/a.md"),
        )
        .unwrap();
        assert_eq!(
            changes,
            vec![
                ("[[x/a]]".to_string(), "[[y/a]]".to_string()),
                ("[[x/a.md]]".to_string(), "[[y/a.md]]".to_string()),
            ]
        );
    }

    /// A file move's rewrite applied unconditionally, the content whether or not
    /// it changed.
    fn rewrite_links(
        content: &str,
        spans: &[ByteRange],
        old_rel: &Path,
        new_rel: &Path,
    ) -> Result<String, MutationReject> {
        apply_edits(content, link_edits(content, spans, old_rel, new_rel)?)
    }

    fn span_of(content: &str, needle: &str) -> ByteRange {
        let start = content.find(needle).expect("needle present");
        ByteRange::new(start, start + needle.len())
    }

    fn moved(content: &str, needles: &[&str], old_rel: &str, new_rel: &str) -> Option<String> {
        let spans: Vec<ByteRange> = needles.iter().map(|n| span_of(content, n)).collect();
        move_rewrite(
            content,
            &spans,
            &PathBuf::from(old_rel),
            &PathBuf::from(new_rel),
        )
        .unwrap()
    }

    #[test]
    fn several_moved_targets_rewrite_in_one_pass() {
        // A folder move: one referrer points at two moved files, path-addressed,
        // plus a bare-name link that stays byte-identical and a pin that freezes.
        let content = "[[drafts/a]] and [[drafts/sub/b.md]], [[a]], [[drafts/a::@abc1]]\n";
        let a = [span_of(content, "[[drafts/a]]"), span_of(content, "[[a]]")];
        let b = [span_of(content, "[[drafts/sub/b.md]]")];
        let targets = [
            MovedTarget {
                spans: &a,
                old_rel: Path::new("drafts/a.md"),
                new_rel: Path::new("notes/a.md"),
            },
            MovedTarget {
                spans: &b,
                old_rel: Path::new("drafts/sub/b.md"),
                new_rel: Path::new("notes/sub/b.md"),
            },
        ];
        assert_eq!(
            moves_rewrite(content, &targets).unwrap().as_deref(),
            Some("[[notes/a]] and [[notes/sub/b.md]], [[a]], [[drafts/a::@abc1]]\n")
        );
        assert_eq!(
            moves_link_changes(content, &targets).unwrap(),
            vec![
                ("[[drafts/a]]".to_string(), "[[notes/a]]".to_string()),
                (
                    "[[drafts/sub/b.md]]".to_string(),
                    "[[notes/sub/b.md]]".to_string()
                ),
            ],
            "document order, identical and frozen links absent"
        );
    }

    #[test]
    fn several_moved_targets_all_bare_leave_the_referrer_unwritten() {
        let content = "[[a]] [[b]]\n";
        let a = [span_of(content, "[[a]]")];
        let b = [span_of(content, "[[b]]")];
        let targets = [
            MovedTarget {
                spans: &a,
                old_rel: Path::new("d/a.md"),
                new_rel: Path::new("e/a.md"),
            },
            MovedTarget {
                spans: &b,
                old_rel: Path::new("d/b.md"),
                new_rel: Path::new("e/b.md"),
            },
        ];
        assert_eq!(moves_rewrite(content, &targets).unwrap(), None);
    }

    #[test]
    fn a_pure_move_leaves_a_bare_name_referrer_unchanged() {
        assert_eq!(moved("see [[a]]\n", &["[[a]]"], "a.md", "sub/a.md"), None);
        assert_eq!(
            moved("see [[a.md]]\n", &["[[a.md]]"], "x/a.md", "y/a.md"),
            None
        );
    }

    #[test]
    fn a_top_level_files_basename_spelling_stays_a_basename_on_a_move() {
        // At the root, `a.md` is both the relpath and the basename, but a link
        // without `/` resolves by name, so the move leaves it byte-identical.
        assert_eq!(
            moved("see [[a.md]]\n", &["[[a.md]]"], "a.md", "sub/a.md"),
            None
        );
        // A rename still re-renders it, in the basename mode.
        assert_eq!(
            moved("see [[a.md]]\n", &["[[a.md]]"], "a.md", "b.md").as_deref(),
            Some("see [[b.md]]\n")
        );
    }

    #[test]
    fn a_pure_move_changes_a_path_addressed_referrer() {
        assert_eq!(
            moved("see [[x/a]]\n", &["[[x/a]]"], "x/a.md", "y/a.md").as_deref(),
            Some("see [[y/a]]\n")
        );
    }

    #[test]
    fn one_changed_reference_makes_the_referrer_changed() {
        let content = "[[a]] and [[x/a]]\n";
        assert_eq!(
            moved(content, &["[[a]]", "[[x/a]]"], "x/a.md", "y/a.md").as_deref(),
            Some("[[a]] and [[y/a]]\n")
        );
    }

    #[test]
    fn a_rename_changes_a_bare_name_referrer() {
        assert_eq!(
            moved("see [[a]]\n", &["[[a]]"], "a.md", "b.md").as_deref(),
            Some("see [[b]]\n")
        );
    }

    /// Rewrite one reference `link` as if renaming `old_rel` to `new_rel`.
    fn one(link: &str, old_rel: &str, new_rel: &str) -> String {
        let span = ByteRange::new(0, link.len());
        rewrite_links(
            link,
            &[span],
            &PathBuf::from(old_rel),
            &PathBuf::from(new_rel),
        )
        .unwrap()
    }

    #[test]
    fn preserves_each_spelling_mode() {
        // Same-directory rename: bare stem, basename, path, and dir-stem each
        // stay in their own mode, the new token re-derived from the new path.
        assert_eq!(one("[[a]]", "a.md", "c.md"), "[[c]]");
        assert_eq!(one("[[a.md]]", "a.md", "c.md"), "[[c.md]]");
        assert_eq!(
            one("[[notes/a]]", "notes/a.md", "notes/c.md"),
            "[[notes/c]]"
        );
        assert_eq!(
            one("[[notes/a.md]]", "notes/a.md", "notes/c.md"),
            "[[notes/c.md]]"
        );
    }

    #[test]
    fn rewrites_a_type_name_spelling_for_a_def_file_move() {
        // A def-ref value `[[mcp.tool]]` spells the type-name, the `.type` tail
        // stripped. A def-file rename must re-derive the new type-name, not
        // normalise to the new stem `mcp.device.type`.
        assert_eq!(
            one(
                "[[mcp.tool]]",
                "type/mcp.tool.type.yaml",
                "type/mcp.device.type.yaml"
            ),
            "[[mcp.device]]"
        );
        // The stem spelling stays the stem; the two modes are distinct.
        assert_eq!(
            one(
                "[[mcp.tool.type]]",
                "type/mcp.tool.type.yaml",
                "type/mcp.device.type.yaml"
            ),
            "[[mcp.device.type]]"
        );
    }

    #[test]
    fn preserves_fragments_and_repo_qualifier() {
        // Anchor, block-id, field, and the repo qualifier all survive; only the
        // target name changes, in the bare-stem mode here.
        let content = "x [[a#head]] [[a^b1]] [[a:role]] [[a::base]] y";
        let spans = [
            span_of(content, "[[a#head]]"),
            span_of(content, "[[a^b1]]"),
            span_of(content, "[[a:role]]"),
            span_of(content, "[[a::base]]"),
        ];
        let out = rewrite_links(
            content,
            &spans,
            &PathBuf::from("a.md"),
            &PathBuf::from("c.md"),
        )
        .unwrap();
        assert_eq!(out, "x [[c#head]] [[c^b1]] [[c:role]] [[c::base]] y");
    }

    #[test]
    fn freezes_a_commit_pin_through_a_rename() {
        // A `@commit` pin names its target as of that commit, so a rename leaves
        // it byte-identical rather than re-pointing it. Both the `::repo@commit`
        // and the this-repo `::@commit` forms freeze, and a fragment on a pin
        // freezes with it — the block carried that id at that commit too.
        let content =
            "[[a::base@a1b2c3d]] [[a::@a1b2c3d]] [[a::base@d4e5f6a#head]] [[a::@d4e5f6a^^rec]]";
        let spans = [
            span_of(content, "[[a::base@a1b2c3d]]"),
            span_of(content, "[[a::@a1b2c3d]]"),
            span_of(content, "[[a::base@d4e5f6a#head]]"),
            span_of(content, "[[a::@d4e5f6a^^rec]]"),
        ];
        let out = rewrite_links(
            content,
            &spans,
            &PathBuf::from("a.md"),
            &PathBuf::from("c.md"),
        )
        .unwrap();
        assert_eq!(out, content);
    }

    #[test]
    fn freezes_only_the_pinned_reference_beside_an_unpinned_one() {
        // The guard is per-reference, not per-file: an unpinned edge in the same
        // file is rewritten in the same pass. Otherwise one pin would freeze a
        // whole referrer and leave its live links dangling.
        let content = "[[a]] then [[a::@a1b2c3d]] then [[notes/a]]";
        let spans = [
            span_of(content, "[[a]]"),
            span_of(content, "[[a::@a1b2c3d]]"),
            span_of(content, "[[notes/a]]"),
        ];
        let out = rewrite_links(
            content,
            &spans,
            &PathBuf::from("notes/a.md"),
            &PathBuf::from("notes/c.md"),
        )
        .unwrap();
        assert_eq!(out, "[[c]] then [[a::@a1b2c3d]] then [[notes/c]]");
    }

    #[test]
    fn freezes_a_pin_under_every_reference_rewrite_form() {
        // The freeze lives in `ref_edits`, the choke point promote and inline
        // reach through too, so a form-changing transform cannot touch a pin
        // either. The transform here would rewrite target AND block-id; the
        // pinned reference comes back untouched, the unpinned one converted.
        let content = "[[host^rec]] and [[host::@a1b2c3d^rec]]";
        let spans = [
            span_of(content, "[[host^rec]]"),
            span_of(content, "[[host::@a1b2c3d^rec]]"),
        ];
        let out = rewrite_refs(content, &spans, |w| {
            let mut nw = w.clone();
            nw.target = "newFile".to_string();
            nw.block_id = None;
            nw
        })
        .unwrap();
        assert_eq!(out, "[[newFile]] and [[host::@a1b2c3d^rec]]");
    }

    #[test]
    fn rewrites_multiple_references_last_span_first() {
        let content = "[[a]] middle [[a]]";
        let spans = [span_of(content, "[[a]]"), {
            let second = content.rfind("[[a]]").unwrap();
            ByteRange::new(second, second + 5)
        }];
        let out = rewrite_links(
            content,
            &spans,
            &PathBuf::from("a.md"),
            &PathBuf::from("cc.md"),
        )
        .unwrap();
        assert_eq!(out, "[[cc]] middle [[cc]]");
    }

    #[test]
    fn an_unrecognized_spelling_defaults_to_the_bare_stem() {
        // A target that matches none of the old path's spellings still resolves
        // by getting the new stem.
        assert_eq!(one("[[weird]]", "a.md", "c.md"), "[[c]]");
    }

    #[test]
    fn promote_changes_a_block_ref_to_a_file_ref() {
        // Promote moves a `^:id` record out to its own file: a referrer's
        // `[[host^id]]` becomes `[[newFile]]` — the block-id dropped, the target
        // swapped, the `:field` attribution and the `::repo` qualifier carried.
        // Canonical fragment order is `target::repo#anchor^block_id:field`.
        let content = "x [[host^rec]] y [[host^rec:role]] z [[host::base^rec]] w";
        let spans = [
            span_of(content, "[[host^rec]]"),
            span_of(content, "[[host^rec:role]]"),
            span_of(content, "[[host::base^rec]]"),
        ];
        let out = rewrite_refs(content, &spans, |w| {
            let mut nw = w.clone();
            nw.target = "newFile".to_string();
            nw.block_id = None;
            nw.anchor = None;
            nw
        })
        .unwrap();
        assert_eq!(
            out,
            "x [[newFile]] y [[newFile:role]] z [[newFile::base]] w"
        );
    }

    #[test]
    fn inline_changes_a_file_ref_to_a_block_ref() {
        // Inline folds a file into a host as a `^:id` record: a referrer's
        // `[[file]]` becomes `[[host^^id]]` — a block-referent (doubled caret),
        // so the slot still pulls the record's value. A bare `^id` would be
        // navigational and resolve to the host file instead. The `:field`
        // attribution and the `::repo` qualifier are carried.
        let content = "x [[file]] y [[file:role]] z [[file::base]] w";
        let spans = [
            span_of(content, "[[file]]"),
            span_of(content, "[[file:role]]"),
            span_of(content, "[[file::base]]"),
        ];
        let out = rewrite_refs(content, &spans, |w| {
            let mut nw = w.clone();
            nw.target = "host".to_string();
            nw.block_id = Some(au_references::BlockId {
                id: "b-1".to_string(),
                referent: true,
            });
            nw
        })
        .unwrap();
        assert_eq!(
            out,
            "x [[host^^b-1]] y [[host^^b-1:role]] z [[host::base^^b-1]] w"
        );
    }

    #[test]
    fn serialize_preserves_the_block_id_mode() {
        // A rewrite round-trips the sigil: a navigational `^id` and a
        // block-referent `^^id` each serialize back to their own form, so no
        // refactor silently flips the mode.
        for raw in [
            "[[note^id]]",
            "[[note^^id]]",
            "[[note^^id:field]]",
            "[[note#head^^id]]",
            "[[^^id]]",
            "[[note::base^^id]]",
            "[[note::base@a1b2c3d^^id:field]]",
        ] {
            let w = parse_wikilink(raw).unwrap();
            assert_eq!(serialize_wikilink(&w), raw, "round-trip changed {raw}");
        }
    }
}
