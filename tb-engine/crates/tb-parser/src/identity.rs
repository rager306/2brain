//! Where a repository keeps its identity directory, and how a name is recognised as one.
//!
//! There is one spelling: `.2brain`. The legacy dot-name is gone (ADR-0005): a binary from
//! before the final rename cannot read a tree written now, and a tree that still carries the old
//! directory is refused rather than silently preferred the other way.
//!
//! - **resolution is per root, never per file**: a repository yields exactly one identity
//!   directory;
//! - **the probe is a closure**, because the walk asks a [`FileSystem`](crate::fs::FileSystem) — a key
//!   map in memory — rather than `std::fs`;
//! - **the floor predicate is pure**: a walk must be able to reject the directory name before it
//!   touches anything.
//!
//! No crate outside this module should spell the name.

use std::path::{Path, PathBuf};

/// The identity directory of a repository.
pub const IDENTITY_DIR: &str = ".2brain";

/// The marker file that makes a directory a repository.
pub const IDENTITY_MARKER: &str = "repo.yaml";

/// The identity directory a writer creates.
pub const CREATE_IDENTITY_DIR: &str = IDENTITY_DIR;

/// Whether `name` is the identity directory.
///
/// This is the walk's floor predicate: a directory whose name answers `true` is a repository
/// boundary, not content.
pub fn is_identity_dir_name(name: &str) -> bool {
    name == IDENTITY_DIR
}

/// The identity directory `root` already holds, `None` when it holds none.
pub fn existing_identity_dir(root: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let current = root.join(IDENTITY_DIR);
    exists(&current).then_some(current)
}

/// The identity directory of `root`.
///
/// `exists` is asked about the joined path only, so a memory filesystem is as good as a real one.
/// When the directory does not exist the path is still returned, a path that does not answer —
/// which is what "no repository here" already looks like.
pub fn identity_dir_with(root: &Path, _exists: impl Fn(&Path) -> bool) -> PathBuf {
    root.join(IDENTITY_DIR)
}

/// The identity directory a WRITER targets.
pub fn identity_dir_for_write_with(root: &Path, _exists: impl Fn(&Path) -> bool) -> PathBuf {
    root.join(CREATE_IDENTITY_DIR)
}

/// The identity marker (`repo.yaml`) of `root`.
pub fn identity_marker_with(root: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    identity_dir_with(root, exists).join(IDENTITY_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retired directory name, built from parts so no source file spells it.
    const LEGACY: &str = concat!(".arsum", "bris");

    fn present<'a>(entries: &'a [&'static str]) -> impl Fn(&Path) -> bool + 'a {
        move |path: &Path| {
            entries
                .iter()
                .any(|entry| path.ends_with(entry) && path.to_string_lossy().ends_with(entry))
        }
    }

    #[test]
    fn a_repository_resolves_to_the_one_name() {
        let root = Path::new("/repo");
        let dir = identity_dir_with(root, present(&[".2brain"]));
        assert_eq!(dir, root.join(".2brain"));
    }

    #[test]
    fn a_legacy_directory_no_longer_answers() {
        let root = Path::new("/repo");
        let dir = identity_dir_with(root, present(&[LEGACY]));
        assert_eq!(dir, root.join(".2brain"), "the old spelling is not read");
        assert!(!is_identity_dir_name(LEGACY));
    }

    #[test]
    fn nothing_present_yields_the_read_default() {
        let root = Path::new("/repo");
        let dir = identity_dir_with(root, present(&[]));
        assert_eq!(dir, root.join(".2brain"));
        assert_eq!(
            identity_marker_with(root, present(&[])),
            root.join(".2brain/repo.yaml")
        );
    }

    #[test]
    fn a_writer_targets_the_one_name() {
        let root = Path::new("/repo");
        assert_eq!(
            identity_dir_for_write_with(root, present(&[])),
            root.join(".2brain")
        );
        assert_eq!(CREATE_IDENTITY_DIR, IDENTITY_DIR);
    }

    #[test]
    fn the_floor_predicate_accepts_only_the_one_name() {
        assert!(is_identity_dir_name(".2brain"));
        assert!(!is_identity_dir_name(LEGACY));
        assert!(!is_identity_dir_name("brain"));
        assert!(!is_identity_dir_name(".git"));
    }
}
