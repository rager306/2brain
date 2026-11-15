//! Planning for the directory mutation verbs per
//! [[spec - directory mutation verbs - move_dir and delete_dir fold a subtree into one per-file saga]].
//!
//! A folder op acts on exactly the files the engine catalogues under the folder.
//! [`plan_subtree`] enumerates them from the catalog and checks the disk holds
//! nothing else, refusing when it does. [`plan_move_dir`] maps that subtree onto
//! its destination. Both are shared by the committing verb and its preview, so
//! the two refuse alike.
//!
//! Planning only: nothing here writes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use tb_diagnostics::ByteRange;

use tb_parser::{FLOOR_DIR_NAMES, RealFileSystem, WalkFilter};

use crate::ir::{ContentHash, KnowledgeBase};
use crate::mutate::MutationReject;
use crate::rename::MovedTarget;

/// A folder's subtree, as the engine catalogues it and the disk confirms.
pub(crate) struct Subtree {
    /// The root of the one editable repo holding the folder.
    pub repo_root: PathBuf,
    /// The folder, absolute.
    pub dir: PathBuf,
    /// Every catalogued file under the folder, path-sorted. Every file kind:
    /// instances, type-defs, and never-read assets alike.
    pub files: Vec<PathBuf>,
    /// Every directory under the folder, the folder itself first, empty ones
    /// included, path-sorted. The skeleton git cannot see.
    pub dirs: Vec<PathBuf>,
    /// The owning member's walk filter, the scope the catalog was built under.
    filter: WalkFilter,
}

impl std::fmt::Debug for Subtree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subtree")
            .field("repo_root", &self.repo_root)
            .field("dir", &self.dir)
            .field("files", &self.files)
            .field("dirs", &self.dirs)
            .finish_non_exhaustive()
    }
}

/// A folder move: the subtree plus the destination of every file and directory.
#[derive(Debug)]
pub(crate) struct MoveDirPlan {
    pub subtree: Subtree,
    /// The destination folder, absolute.
    pub to: PathBuf,
    /// Each catalogued file's `(old, new)` path, path-sorted by `old`.
    pub files: Vec<(PathBuf, PathBuf)>,
    /// Each directory's `(old, new)` path, path-sorted by `old`, the folder first.
    pub dirs: Vec<(PathBuf, PathBuf)>,
    /// Each moved file's repo-relative `(old, new)` path, keyed by its absolute
    /// old path: the spellings a referrer's links are re-rendered between.
    rels: BTreeMap<PathBuf, (PathBuf, PathBuf)>,
    /// Every file referencing a moved file, moved files included, each with the
    /// spans of its references keyed by the moved file they resolve to. From
    /// the backlink index, which locates; the disk is re-read at write time.
    pub referrers: BTreeMap<PathBuf, BTreeMap<PathBuf, Vec<ByteRange>>>,
    /// The content hash each referrer's spans were read against.
    pub referrer_hashes: BTreeMap<PathBuf, Option<ContentHash>>,
}

/// Plan the subtree of the folder `dir`, or the refusal message.
///
/// `dir` is absolute and resolved. Refuses:
/// - a folder not in one editable repo, a repo root, or under a hard-floor
///   directory (`.git`, `.arsumbris`).
/// - a folder holding a workspace member's root, a nested repo moved file by
///   file would drop out of the workspace.
/// - a folder whose disk holds anything outside the catalog: a scoped-out path,
///   a nested `.git` or `.arsumbris`, a nested repo, a symlink, or an entry the
///   walk could not read. Moving only the catalogued files would leave the rest
///   behind; carrying it along would move bytes no plan describes.
/// - a folder whose disk files differ from the catalog's, an engine view
///   behind the disk. Retrying after the rebuild settles resolves it.
/// - a folder that is a symlink, or sits under one. Git passes a path beyond a
///   symlink as clean and then refuses to stage it, so the op could neither
///   commit nor roll back: a move would lose the files, a delete would remove
///   the link target's files with no commit.
pub(crate) fn plan_subtree(v: &KnowledgeBase, dir: &Path) -> Result<Subtree, String> {
    let repo = v
        .repos
        .repo_of(dir)
        .ok_or_else(|| format!("path escapes the workspace: {}", dir.display()))?;
    let role = crate::wire::member_role(v, &repo.name);
    if !role.editable() {
        return Err(format!(
            "member '{}' is mounted as '{}', not editable — list it in workspace.yaml 'edit:' to author it",
            repo.name.as_str(),
            role.as_str(),
        ));
    }
    let repo_root = repo.root.clone();
    if dir == repo_root {
        return Err("path is a repo root; a folder op acts on a folder inside a repo".to_string());
    }
    if under_floor(&repo_root, dir) {
        return Err(format!(
            "path is under an engine-owned or git directory: {}",
            dir.display()
        ));
    }
    if let Some(link) = symlinked_component(&repo_root, dir) {
        return Err(format!(
            "the folder is reached through a symlink, which git can neither stage nor restore beyond: {} (symlink)",
            rel(&repo_root, &link)
        ));
    }

    let nested: Vec<String> = v
        .repos
        .repos()
        .iter()
        .filter(|r| !r.builtin && r.root != repo_root && r.root.starts_with(dir))
        .map(|r| format!("{} ({})", rel(&repo_root, &r.root), r.name.as_str()))
        .collect();
    if !nested.is_empty() {
        return Err(format!(
            "the folder holds a nested repo, which a folder op cannot carry: {}",
            nested.join(", ")
        ));
    }

    let mut diags = Vec::new();
    let (filter, _) = crate::build::member_walk_filter(&repo_root, &RealFileSystem, &mut diags);
    // The audit starts AT the folder and asks the filter only about what lies
    // below it, so a scoped-out folder holding no files would audit clean.
    if !filter.enter_dir(dir) {
        return Err(format!(
            "the folder holds content the engine does not catalogue, so it cannot move or delete it whole: {} (scoped out)",
            rel(&repo_root, dir)
        ));
    }
    let depth = dir
        .strip_prefix(&repo_root)
        .map_or(0, |r| r.components().count());
    let audit = tb_parser::audit_subtree(dir, &filter, depth).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("path does not exist: {}", dir.display()),
        std::io::ErrorKind::InvalidInput if !dir.is_dir() => {
            format!("path is not a directory: {}", dir.display())
        }
        _ => format!("cannot walk {}: {e}", dir.display()),
    })?;

    let mut outside: Vec<String> = Vec::new();
    let mut note = |label: &str, paths: &[PathBuf]| {
        for p in paths {
            outside.push(format!("{} ({label})", rel(&repo_root, p)));
        }
    };
    note("scoped out", &audit.pruned_dirs);
    note("scoped out", &audit.dropped_files);
    note("git or engine directory", &audit.floored_dirs);
    let markers: Vec<PathBuf> = audit
        .repo_markers
        .iter()
        .filter_map(|m| m.parent().and_then(Path::parent).map(Path::to_path_buf))
        .collect();
    note("nested repo", &markers);
    note("symlink", &audit.symlinks);
    let unreadable: Vec<PathBuf> = audit.errors.iter().map(|e| e.path.clone()).collect();
    note("unreadable", &unreadable);
    if !outside.is_empty() {
        outside.sort();
        return Err(format!(
            "the folder holds content the engine does not catalogue, so it cannot move or delete it whole: {}",
            outside.join(", ")
        ));
    }

    let files: Vec<PathBuf> = v
        .catalog
        .range(dir.to_path_buf()..)
        .take_while(|(p, _)| p.starts_with(dir))
        .map(|(p, _)| p.clone())
        .collect();
    if files != audit.files {
        let mut stale: Vec<String> = audit
            .files
            .iter()
            .filter(|p| files.binary_search(p).is_err())
            .map(|p| format!("{} (on disk, not yet catalogued)", rel(&repo_root, p)))
            .chain(
                files
                    .iter()
                    .filter(|p| audit.files.binary_search(p).is_err())
                    .map(|p| format!("{} (catalogued, gone from disk)", rel(&repo_root, p))),
            )
            .collect();
        stale.sort();
        return Err(format!(
            "the engine's view of the folder is behind the disk, retry once the rebuild settles: {}",
            stale.join(", ")
        ));
    }

    Ok(Subtree {
        repo_root,
        dir: dir.to_path_buf(),
        files,
        dirs: audit.dirs,
        filter,
    })
}

/// Plan a move of the folder `dir` to `to`, or the refusal message.
///
/// Both paths absolute and resolved. Refuses everything [`plan_subtree`] does,
/// plus a destination that is:
/// - taken, an existing file or directory, or one colliding case-insensitively
///   with a sibling. The verb never merges into an existing folder.
/// - the folder itself or inside it.
/// - in a different repo, or under a hard-floor directory.
/// - out of the repo's scope, a moved file or directory the member's walk
///   would not keep. It would drop out of the catalog, stranding every
///   reference rewritten to it.
///
/// Every file keeps its basename, so a type-def moves under its own name.
pub(crate) fn plan_move_dir(
    v: &KnowledgeBase,
    dir: &Path,
    to: &Path,
) -> Result<MoveDirPlan, String> {
    if to == dir || to.starts_with(dir) {
        return Err("destination is the folder itself or inside it".to_string());
    }
    let subtree = plan_subtree(v, dir)?;
    match v.repos.repo_of(to) {
        Some(r) if r.root == subtree.repo_root => {}
        Some(_) => {
            return Err(
                "cross-repo folder move is not supported — path and to must be in the same repo"
                    .to_string(),
            );
        }
        None => {
            return Err(format!(
                "destination escapes the workspace: {}",
                to.display()
            ));
        }
    }
    if to == subtree.repo_root || under_floor(&subtree.repo_root, to) {
        return Err(format!(
            "destination is a repo root or under an engine-owned or git directory: {}",
            to.display()
        ));
    }
    check_destination_ancestors(&subtree.repo_root, to)?;
    if to.symlink_metadata().is_ok() {
        return Err(format!("destination already exists: {}", to.display()));
    }
    if let (Some(parent), Some(name)) = (to.parent(), to.file_name()) {
        let wanted = name.to_string_lossy().to_lowercase();
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().to_lowercase() == wanted {
                    return Err(format!(
                        "destination collides case-insensitively with an existing entry: {}",
                        entry.file_name().to_string_lossy()
                    ));
                }
            }
        }
    }

    let map = |p: &Path| {
        to.join(
            p.strip_prefix(dir)
                .expect("subtree paths lie under the folder"),
        )
    };
    let files: Vec<(PathBuf, PathBuf)> =
        subtree.files.iter().map(|p| (p.clone(), map(p))).collect();
    let dirs: Vec<(PathBuf, PathBuf)> = subtree.dirs.iter().map(|p| (p.clone(), map(p))).collect();

    let mut scoped_out: Vec<String> = files
        .iter()
        .filter(|(_, new)| !subtree.filter.keep_file(new))
        .map(|(_, new)| rel(&subtree.repo_root, new))
        .chain(
            dirs.iter()
                .filter(|(_, new)| !subtree.filter.enter_dir(new))
                .map(|(_, new)| rel(&subtree.repo_root, new)),
        )
        .collect();
    if !scoped_out.is_empty() {
        scoped_out.sort();
        scoped_out.dedup();
        return Err(format!(
            "the destination is out of the repo's scope, the moved content would drop out of the graph: {}",
            scoped_out.join(", ")
        ));
    }

    let rels: BTreeMap<PathBuf, (PathBuf, PathBuf)> = files
        .iter()
        .map(|(old, new)| {
            (
                old.clone(),
                (
                    rel_path(&subtree.repo_root, old),
                    rel_path(&subtree.repo_root, new),
                ),
            )
        })
        .collect();
    let mut referrers: BTreeMap<PathBuf, BTreeMap<PathBuf, Vec<ByteRange>>> = BTreeMap::new();
    let mut referrer_hashes: BTreeMap<PathBuf, Option<ContentHash>> = BTreeMap::new();
    for (old, _) in &files {
        for bl in v.backlinks(old) {
            referrers
                .entry(bl.source.clone())
                .or_default()
                .entry(old.clone())
                .or_default()
                .push(bl.span);
            referrer_hashes
                .entry(bl.source.clone())
                .or_insert_with(|| v.catalog.get(&bl.source).and_then(|e| e.hash));
        }
    }

    Ok(MoveDirPlan {
        subtree,
        to: to.to_path_buf(),
        files,
        dirs,
        rels,
        referrers,
        referrer_hashes,
    })
}

impl MoveDirPlan {
    /// The moved files `referrer` points at, as the rewrite seam takes them.
    fn targets(&self, referrer: &Path) -> Vec<MovedTarget<'_>> {
        self.referrers
            .get(referrer)
            .map(|by_file| {
                by_file
                    .iter()
                    .filter_map(|(old, spans)| {
                        let (old_rel, new_rel) = self.rels.get(old)?;
                        Some(MovedTarget {
                            spans,
                            old_rel,
                            new_rel,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `referrer`'s content after the move, `None` when its bytes do not change.
    /// Every moved file it points at is re-pointed in one pass, see
    /// [`crate::rename::moves_rewrite`].
    pub fn rewrite(
        &self,
        referrer: &Path,
        content: &str,
    ) -> Result<Option<String>, MutationReject> {
        crate::rename::moves_rewrite(content, &self.targets(referrer))
    }

    /// The links the move re-spells in `referrer`, `(before, after)`.
    pub fn link_changes(
        &self,
        referrer: &Path,
        content: &str,
    ) -> Result<Vec<(String, String)>, MutationReject> {
        crate::rename::moves_link_changes(content, &self.targets(referrer))
    }

    /// Whether `path` is one of the moved files, at its old path.
    pub fn moves(&self, path: &Path) -> bool {
        self.rels.contains_key(path)
    }

    /// The referrers the move leaves byte-identical, so it does not write them:
    /// no clean gate, no commit, no rollback. Still recomputed.
    ///
    /// Conservative, each doubtful case stays written, so it stays gated: an
    /// unreadable referrer, one whose bytes drifted from the held hash (its
    /// spans are unreliable), a rewrite error. A moved file is never
    /// not-written, the move deletes it at its old path.
    pub fn not_written(&self) -> BTreeSet<PathBuf> {
        self.referrers
            .keys()
            .filter(|path| !self.moves(path))
            .filter(|path| {
                let Ok(content) = std::fs::read_to_string(path) else {
                    return false;
                };
                self.referrer_hashes.get(*path).copied().flatten()
                    == Some(ContentHash::of(content.as_bytes()))
                    && matches!(self.rewrite(path, &content), Ok(None))
            })
            .cloned()
            .collect()
    }

    /// Every path the move touches: each moved file at both ends, and every
    /// referrer.
    pub fn touched(&self) -> Vec<PathBuf> {
        let mut out: BTreeSet<PathBuf> = BTreeSet::new();
        for (old, new) in &self.files {
            out.insert(old.clone());
            out.insert(new.clone());
        }
        out.extend(self.referrers.keys().cloned());
        out.into_iter().collect()
    }

    /// Each moved file's repo-relative `(old, new)` path, for the `Moved:`
    /// trailers.
    pub fn moved_rels(&self) -> impl Iterator<Item = &(PathBuf, PathBuf)> {
        self.rels.values()
    }

    /// The directories the move creates, deepest last: every mapped directory,
    /// plus each missing ancestor of the destination.
    pub fn created_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.dirs.iter().map(|(_, new)| new.clone()).collect();
        let mut up = self.to.parent();
        while let Some(dir) = up {
            if dir == self.subtree.repo_root || dir.exists() {
                break;
            }
            out.push(dir.to_path_buf());
            up = dir.parent();
        }
        out.sort();
        out
    }

    /// The directories a rollback must leave in place: every source directory
    /// the move removes, and the destination's nearest existing ancestor, which
    /// the rollback's pruning of a created file could otherwise take.
    pub fn kept_dirs(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self.dirs.iter().map(|(old, _)| old.clone()).collect();
        let mut up = self.to.parent();
        while let Some(dir) = up {
            if dir.exists() {
                out.push(dir.to_path_buf());
                break;
            }
            up = dir.parent();
        }
        out.sort();
        out
    }

    /// The created directories no commit records: each holding no file that
    /// commits, empty or holding only files written outside git.
    pub fn untracked_dirs(&self, outside_git: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
        self.created_dirs()
            .into_iter()
            .filter(|dir| {
                !self
                    .files
                    .iter()
                    .any(|(old, new)| new.starts_with(dir) && !outside_git.contains(old))
            })
            .collect()
    }
}

/// A folder delete: the subtree, and the references its removal strands.
#[derive(Debug)]
pub(crate) struct DeleteDirPlan {
    pub subtree: Subtree,
    /// Every reference from a SURVIVING file into a contained file, grouped by
    /// referrer, path-sorted. A reference between two contained files is absent,
    /// both are gone.
    pub stranded: Vec<crate::wire::StrandedView>,
}

/// Plan a delete of the folder `dir`, or the refusal message.
///
/// Refuses everything [`plan_subtree`] does. The stranded report is read from
/// the held backlink index and each referrer's held parse, report-only: a
/// delete rewrites no referrer, so none is read from disk or gated.
pub(crate) fn plan_delete_dir(v: &KnowledgeBase, dir: &Path) -> Result<DeleteDirPlan, String> {
    let subtree = plan_subtree(v, dir)?;
    let contained: BTreeSet<&PathBuf> = subtree.files.iter().collect();
    let referrers: BTreeSet<PathBuf> = subtree
        .files
        .iter()
        .flat_map(|f| v.backlinks(f))
        .map(|bl| bl.source.clone())
        .filter(|src| !contained.contains(src))
        .collect();
    let mut stranded = Vec::new();
    for source in referrers {
        let Some(entry) = v.catalog.get(&source) else {
            continue;
        };
        let links: Vec<crate::wire::StrandedLinkView> = crate::backlinks::walk_edges(
            &source,
            &entry.parse,
            &v.indexes,
            &v.repos,
            &v.workspaces,
        )
        .into_iter()
        .filter_map(|edge| {
            let to_file = edge.resolved?;
            contained
                .contains(&to_file)
                .then(|| crate::wire::StrandedLinkView {
                    target: crate::rename::serialize_wikilink(&edge.link),
                    to_file: to_file.display().to_string(),
                })
        })
        .collect();
        if !links.is_empty() {
            stranded.push(crate::wire::StrandedView {
                path: source.display().to_string(),
                links,
            });
        }
    }
    Ok(DeleteDirPlan { subtree, stranded })
}

impl DeleteDirPlan {
    /// The directories a rollback must put back: every directory the delete
    /// removes, empty ones included.
    pub fn kept_dirs(&self) -> Vec<PathBuf> {
        self.subtree.dirs.clone()
    }

    /// The removed directories no commit records: each holding no file that
    /// commits, empty or holding only files outside git.
    pub fn untracked_dirs(&self, outside_git: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
        self.subtree
            .dirs
            .iter()
            .filter(|dir| {
                !self
                    .subtree
                    .files
                    .iter()
                    .any(|f| f.starts_with(dir) && !outside_git.contains(f))
            })
            .cloned()
            .collect()
    }
}

/// The first component from below `repo_root` down to `path`, inclusive, that
/// is a symlink. `None` when there is none, or the path stops existing first.
fn symlinked_component(repo_root: &Path, path: &Path) -> Option<PathBuf> {
    let mut at = repo_root.to_path_buf();
    for c in path.strip_prefix(repo_root).ok()?.components() {
        at.push(c);
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => return Some(at),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

/// Refuse a destination whose directories the graph cannot hold, over every
/// ancestor of `to` below `repo_root`. The leaf itself is the caller's: taken,
/// or colliding with a sibling.
/// - a symlink: git can neither stage nor restore a path beyond one.
/// - a nested repo marker: the build walk stops there, so the moved content
///   would drop out of the catalog and every reference rewritten to it dangle.
/// - a directory spelled differently from an existing sibling only by case: a
///   case-insensitive filesystem lands the content in the sibling while the
///   engine records the new spelling. Refused on every filesystem, as the leaf.
///
/// Shared by a folder move and a file rename, so the two refuse alike.
pub(crate) fn check_destination_ancestors(repo_root: &Path, to: &Path) -> Result<(), String> {
    let Some(parent) = to.parent() else {
        return Ok(());
    };
    let Ok(rel_parent) = parent.strip_prefix(repo_root) else {
        return Ok(());
    };
    let mut at = repo_root.to_path_buf();
    for c in rel_parent.components() {
        let Component::Normal(name) = c else {
            continue;
        };
        let wanted = name.to_string_lossy().to_lowercase();
        if let Ok(entries) = std::fs::read_dir(&at) {
            for entry in entries.flatten() {
                let existing = entry.file_name();
                if existing != name && existing.to_string_lossy().to_lowercase() == wanted {
                    return Err(format!(
                        "destination collides case-insensitively with an existing entry: {}",
                        rel(repo_root, &at.join(&existing))
                    ));
                }
            }
        }
        at.push(name);
        match at.symlink_metadata() {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "destination is reached through a symlink, which git can neither stage nor restore beyond: {} (symlink)",
                    rel(repo_root, &at)
                ));
            }
            Ok(m) if m.is_dir() && at.join(".arsumbris/repo.yaml").is_file() => {
                return Err(format!(
                    "destination is inside a nested repo, whose content the graph does not hold: {} (nested repo)",
                    rel(repo_root, &at)
                ));
            }
            Ok(_) => {}
            // The rest of the path is created by the op, nothing to check.
            Err(_) => return Ok(()),
        }
    }
    Ok(())
}

/// Whether `path` lies under a hard-floor directory below `repo_root`.
fn under_floor(repo_root: &Path, path: &Path) -> bool {
    path.strip_prefix(repo_root)
        .map(|r| {
            r.components().any(|c| match c {
                Component::Normal(n) => FLOOR_DIR_NAMES.contains(&n.to_string_lossy().as_ref()),
                _ => false,
            })
        })
        .unwrap_or(false)
}

/// `path` relative to `root`, for a refusal message.
fn rel(root: &Path, path: &Path) -> String {
    rel_path(root, path).display().to_string()
}

/// `path` relative to `root`, the repo-relative spelling a link re-renders.
fn rel_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tb_parser::RealFileSystem;

    /// A folder-repo `v` under a tempdir, with `files` written relative to it.
    fn repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        write(&root, ".arsumbris/repo.yaml", "name: v\n");
        for (p, c) in files {
            write(&root, p, c);
        }
        (tmp, root)
    }

    fn write(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    fn kb(root: &Path) -> KnowledgeBase {
        crate::build::build(root, &RealFileSystem).unwrap()
    }

    #[test]
    fn a_folder_maps_every_catalogued_file_and_directory() {
        let (_t, root) = repo(&[
            ("drafts/a.md", "# a\n"),
            ("drafts/sub/b.png", "png"),
            ("drafts/type/t.type.yaml", "fields: {}\n"),
            ("other.md", "# o\n"),
        ]);
        std::fs::create_dir_all(root.join("drafts/empty")).unwrap();
        let v = kb(&root);
        let plan = plan_move_dir(&v, &root.join("drafts"), &root.join("notes/old")).unwrap();
        let rel = |p: &Path| p.strip_prefix(&root).unwrap().display().to_string();
        let files: Vec<(String, String)> =
            plan.files.iter().map(|(a, b)| (rel(a), rel(b))).collect();
        assert_eq!(
            files,
            vec![
                ("drafts/a.md".into(), "notes/old/a.md".into()),
                ("drafts/sub/b.png".into(), "notes/old/sub/b.png".into()),
                (
                    "drafts/type/t.type.yaml".into(),
                    "notes/old/type/t.type.yaml".into()
                ),
            ],
            "every kind, the type-def included, keeps its basename"
        );
        let dirs: Vec<(String, String)> = plan.dirs.iter().map(|(a, b)| (rel(a), rel(b))).collect();
        assert_eq!(
            dirs,
            vec![
                ("drafts".into(), "notes/old".into()),
                ("drafts/empty".into(), "notes/old/empty".into()),
                ("drafts/sub".into(), "notes/old/sub".into()),
                ("drafts/type".into(), "notes/old/type".into()),
            ],
            "the skeleton includes the empty directory"
        );
    }

    #[test]
    fn a_folder_with_no_files_is_a_valid_plan() {
        let (_t, root) = repo(&[]);
        std::fs::create_dir_all(root.join("drafts/inner")).unwrap();
        let v = kb(&root);
        let plan = plan_move_dir(&v, &root.join("drafts"), &root.join("moved")).unwrap();
        assert!(plan.files.is_empty());
        assert_eq!(plan.dirs.len(), 2);
    }

    #[test]
    fn content_outside_the_catalog_refuses_and_is_named() {
        let (_t, root) = repo(&[("drafts/a.md", "# a\n"), ("drafts/node_modules/x.js", "")]);
        std::fs::create_dir_all(root.join("drafts/clone/.git")).unwrap();
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("drafts")).unwrap_err();
        assert!(err.contains("drafts/node_modules (scoped out)"), "{err}");
        assert!(
            err.contains("drafts/clone/.git (git or engine directory)"),
            "{err}"
        );
    }

    #[test]
    fn a_scoped_out_folder_holding_no_files_refuses() {
        let (_t, root) = repo(&[("a.md", "# a\n")]);
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("node_modules/x")).unwrap_err();
        assert!(err.contains("node_modules/x (scoped out)"), "{err}");
    }

    #[test]
    fn an_auignore_dropped_file_refuses() {
        let (_t, root) = repo(&[
            (".arsumbris/.auignore", "*.log\n"),
            ("drafts/a.md", "# a\n"),
            ("drafts/run.log", ""),
        ]);
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("drafts")).unwrap_err();
        assert!(err.contains("drafts/run.log (scoped out)"), "{err}");
    }

    #[test]
    fn an_undeclared_nested_repo_refuses() {
        let (_t, root) = repo(&[
            ("drafts/a.md", "# a\n"),
            ("drafts/inner/.arsumbris/repo.yaml", "name: inner\n"),
        ]);
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("drafts")).unwrap_err();
        assert!(err.contains("drafts/inner (nested repo)"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_refuses() {
        let (_t, root) = repo(&[("drafts/a.md", "# a\n"), ("elsewhere/t.md", "# t\n")]);
        std::os::unix::fs::symlink(root.join("elsewhere"), root.join("drafts/link")).unwrap();
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("drafts")).unwrap_err();
        assert!(err.contains("drafts/link (symlink)"), "{err}");
    }

    #[test]
    fn a_file_created_after_the_build_refuses_as_stale() {
        let (_t, root) = repo(&[("drafts/a.md", "# a\n")]);
        let v = kb(&root);
        write(&root, "drafts/late.md", "# late\n");
        let err = plan_subtree(&v, &root.join("drafts")).unwrap_err();
        assert!(
            err.contains("drafts/late.md (on disk, not yet catalogued)"),
            "{err}"
        );
    }

    #[test]
    fn repo_root_floor_and_missing_paths_refuse() {
        let (_t, root) = repo(&[("drafts/a.md", "# a\n"), ("f.md", "# f\n")]);
        let v = kb(&root);
        assert!(plan_subtree(&v, &root).unwrap_err().contains("repo root"));
        assert!(
            plan_subtree(&v, &root.join(".arsumbris"))
                .unwrap_err()
                .contains("engine-owned")
        );
        assert!(
            plan_subtree(&v, &root.join("nope"))
                .unwrap_err()
                .contains("does not exist")
        );
        assert!(
            plan_subtree(&v, &root.join("f.md"))
                .unwrap_err()
                .contains("not a directory")
        );
    }

    #[test]
    fn a_taken_or_self_nested_destination_refuses() {
        let (_t, root) = repo(&[("drafts/a.md", "# a\n"), ("Notes/n.md", "# n\n")]);
        let v = kb(&root);
        let dir = root.join("drafts");
        assert!(
            plan_move_dir(&v, &dir, &root.join("Notes"))
                .unwrap_err()
                .contains("already exists")
        );
        assert!(
            plan_move_dir(&v, &dir, &root.join("drafts/deeper"))
                .unwrap_err()
                .contains("inside it")
        );
        assert!(
            plan_move_dir(&v, &dir, &dir)
                .unwrap_err()
                .contains("itself")
        );
        assert!(
            plan_move_dir(&v, &dir, &root.join(".arsumbris/x"))
                .unwrap_err()
                .contains("engine-owned")
        );
    }

    #[test]
    fn a_case_colliding_destination_refuses() {
        let (_t, root) = repo(&[("drafts/a.md", "# a\n"), ("Notes/n.md", "# n\n")]);
        let v = kb(&root);
        // On a case-sensitive filesystem `notes` does not exist, so only the
        // sibling scan catches it; on APFS it also exists. Either way it refuses.
        let err = plan_move_dir(&v, &root.join("drafts"), &root.join("notes")).unwrap_err();
        assert!(
            err.contains("case-insensitively") || err.contains("already exists"),
            "{err}"
        );
    }

    /// An entry `v` editing a nested member `inner`, and depending on a nested
    /// `lib`, a consumed member.
    fn workspace() -> (tempfile::TempDir, PathBuf) {
        let (t, root) = repo(&[
            (".arsumbris/workspace.yaml", "edit:\n  - v\n  - inner\n"),
            ("drafts/a.md", "# a\n"),
            ("drafts/inner/.arsumbris/repo.yaml", "name: inner\n"),
            ("drafts/inner/i.md", "# i\n"),
            ("vendor/lib/.arsumbris/repo.yaml", "name: lib\n"),
            ("vendor/lib/docs/l.md", "# l\n"),
            ("other/o.md", "# o\n"),
        ]);
        write(
            &root,
            ".arsumbris/repo.yaml",
            "name: v\ndeps:\n  - name: lib\n",
        );
        (t, root)
    }

    #[test]
    fn a_declared_member_under_the_folder_refuses() {
        let (_t, root) = workspace();
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("drafts")).unwrap_err();
        assert!(err.contains("drafts/inner (inner)"), "{err}");
    }

    #[test]
    fn a_destination_in_another_repo_refuses() {
        let (_t, root) = workspace();
        let v = kb(&root);
        let err =
            plan_move_dir(&v, &root.join("other"), &root.join("drafts/inner/other")).unwrap_err();
        assert!(err.contains("cross-repo"), "{err}");
    }

    #[test]
    fn a_folder_in_a_consumed_member_refuses() {
        let (_t, root) = workspace();
        let v = kb(&root);
        let err = plan_subtree(&v, &root.join("vendor/lib/docs")).unwrap_err();
        assert!(err.contains("not editable"), "{err}");
    }

    #[test]
    fn a_destination_out_of_scope_refuses() {
        let (_t, root) = repo(&[
            (".arsumbris/.auignore", "archive/\n"),
            ("drafts/a.md", "# a\n"),
        ]);
        let v = kb(&root);
        let err =
            plan_move_dir(&v, &root.join("drafts"), &root.join("archive/drafts")).unwrap_err();
        assert!(err.contains("out of the repo's scope"), "{err}");
        assert!(err.contains("archive/drafts/a.md"), "{err}");
    }
}
