//! Where a repository keeps its identity directory, and how a name is recognised as one.
//!
//! Two spellings exist during the rebrand (`docs/research/tier-c-seams.md`): `.2brain` is what a
//! migrated repository holds, `.arsumbris` is what an un-migrated one holds. This module states the
//! rule once so no call site invents its own join:
//!
//! - **resolution is per root, never per file**: the new name wins when it exists at a root, the
//!   legacy name answers only when it does not, so one repository can never yield two identity
//!   directories;
//! - **the probe is a closure**, because the walk asks a [`FileSystem`](crate::fs::FileSystem) — a key
//!   map in memory — rather than `std::fs`;
//! - **the floor predicate is pure**: a walk must be able to reject the directory name before it
//!   touches anything.
//!
//! No crate outside this module should spell either name.

use std::path::{Path, PathBuf};

/// The identity directory of a migrated repository.
pub const IDENTITY_DIR: &str = ".2brain";

/// The identity directory of a repository that has not been migrated yet.
pub const LEGACY_IDENTITY_DIR: &str = ".arsumbris";

/// The marker file that makes a directory a repository.
pub const IDENTITY_MARKER: &str = "repo.yaml";

/// The identity directory a writer creates when the root holds neither spelling.
///
/// Release 1 creates the legacy name, so a previous-release binary still reads what this one
/// writes; release 2 flips it to [`IDENTITY_DIR`] (tier C step 7). A writer that finds an
/// existing directory never consults this — it follows that directory instead.
pub const CREATE_IDENTITY_DIR: &str = LEGACY_IDENTITY_DIR;

/// Whether `name` is an identity directory under either spelling.
///
/// This is the walk's floor predicate: a directory whose name answers `true` is a repository
/// boundary, not content.
pub fn is_identity_dir_name(name: &str) -> bool {
    name == IDENTITY_DIR || name == LEGACY_IDENTITY_DIR
}

/// The identity directory `root` already holds, the new spelling winning, `None` when it holds
/// neither.
pub fn existing_identity_dir(root: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let current = root.join(IDENTITY_DIR);
    if exists(&current) {
        return Some(current);
    }
    let legacy = root.join(LEGACY_IDENTITY_DIR);
    exists(&legacy).then_some(legacy)
}

/// The identity directory of `root`, preferring the new spelling when `exists` says it is there.
///
/// `exists` is asked about the joined paths only, so a memory filesystem is as good as a real one.
/// This is the READ rule. When neither spelling exists the new one is returned, a path that does
/// not answer — which is what "no repository here" already looks like. A caller about to CREATE
/// the directory wants [`identity_dir_for_write_with`] instead.
pub fn identity_dir_with(root: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    existing_identity_dir(root, exists).unwrap_or_else(|| root.join(IDENTITY_DIR))
}

/// The identity directory a WRITER targets: the one `root` already holds, else
/// [`CREATE_IDENTITY_DIR`].
///
/// Follow an existing directory rather than the read rule's default, so a repository keeps one
/// identity directory for its whole life; only a root with neither spelling is new, and what a new
/// one is called is [`CREATE_IDENTITY_DIR`]'s decision, not the caller's.
pub fn identity_dir_for_write_with(root: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    existing_identity_dir(root, exists).unwrap_or_else(|| root.join(CREATE_IDENTITY_DIR))
}

/// The identity marker (`repo.yaml`) of `root`, resolved by the same rule as [`identity_dir_with`].
pub fn identity_marker_with(root: &Path, exists: impl Fn(&Path) -> bool) -> PathBuf {
    identity_dir_with(root, exists).join(IDENTITY_MARKER)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn present<'a>(entries: &'a [&'static str]) -> impl Fn(&Path) -> bool + 'a {
        move |path: &Path| {
            entries
                .iter()
                .any(|entry| path.ends_with(entry) && path.to_string_lossy().ends_with(entry))
        }
    }

    #[test]
    fn a_migrated_repository_resolves_to_the_new_name() {
        let root = Path::new("/repo");
        let dir = identity_dir_with(root, present(&[".2brain"]));
        assert_eq!(dir, root.join(".2brain"));
    }

    #[test]
    fn an_unmigrated_repository_resolves_to_the_legacy_name() {
        let root = Path::new("/repo");
        let dir = identity_dir_with(root, present(&[".arsumbris"]));
        assert_eq!(dir, root.join(".arsumbris"));
    }

    #[test]
    fn both_spellings_present_still_yields_one_directory() {
        let root = Path::new("/repo");
        let dir = identity_dir_with(root, present(&[".2brain", ".arsumbris"]));
        assert_eq!(dir, root.join(".2brain"), "the new spelling wins, per root");
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
    fn a_writer_follows_the_directory_the_root_already_holds() {
        let root = Path::new("/repo");
        assert_eq!(
            identity_dir_for_write_with(root, present(&[".arsumbris"])),
            root.join(".arsumbris")
        );
        assert_eq!(
            identity_dir_for_write_with(root, present(&[".2brain"])),
            root.join(".2brain")
        );
        assert_eq!(
            identity_dir_for_write_with(root, present(&[".2brain", ".arsumbris"])),
            root.join(".2brain"),
            "the same per-root rule decides which one is followed"
        );
    }

    #[test]
    fn a_writer_creates_the_previous_release_can_still_read() {
        let root = Path::new("/repo");
        assert_eq!(CREATE_IDENTITY_DIR, LEGACY_IDENTITY_DIR);
        assert_eq!(
            identity_dir_for_write_with(root, present(&[])),
            root.join(".arsumbris")
        );
    }

    #[test]
    fn the_floor_predicate_accepts_exactly_the_two_spellings() {
        assert!(is_identity_dir_name(".2brain"));
        assert!(is_identity_dir_name(".arsumbris"));
        assert!(!is_identity_dir_name("brain"));
        assert!(!is_identity_dir_name(".git"));
    }
}
