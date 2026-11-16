//! An overlay `FileSystem` for the mutation preview read: an inner filesystem
//! with a set of paths overridden, each replaced with would-be bytes (a write or
//! an edit's product), carried over from another path (a moved file), or removed
//! (a delete). A single-file op overrides one path, a move or a folder op
//! several (each moved file, its source, every changed referrer). The engine's rebuild dispatch reads a
//! dirty path through the `FileSystem` port, so running that dispatch over this
//! overlay computes a mutation's product without writing disk or touching git.
//! See [[spec - mutation preview read - simulate a write over an overlay and
//! report its product without committing]].

use std::io;
use std::path::{Path, PathBuf};

use tb_parser::scope::WalkFilter;
use tb_parser::{FileSystem, ScopeBoundaries, Walk, WalkError};

/// One path's change an overlay applies over its inner filesystem.
pub(crate) enum Override {
    /// A write or an edit product: the path reads as `content`, and an in-scope
    /// add joins the walk.
    Replace { path: PathBuf, content: Vec<u8> },
    /// A move: the path reads as `from` reads on the inner filesystem, and an
    /// in-scope add joins the walk. The bytes are fetched only when the build
    /// asks, so a moved asset, which the build never reads, is never read.
    Moved { path: PathBuf, from: PathBuf },
    /// A delete: the path reads as absent, and the walk excludes it.
    Remove { path: PathBuf },
}

impl Override {
    pub(crate) fn path(&self) -> &Path {
        match self {
            Override::Replace { path, .. }
            | Override::Moved { path, .. }
            | Override::Remove { path } => path,
        }
    }
}

/// A `FileSystem` presenting its inner filesystem with a set of paths
/// overridden. All other paths fall through to the inner, so a preview reuses
/// every unchanged file's real bytes and re-parses only the overridden ones.
pub(crate) struct OverlayFileSystem<'a, F: FileSystem> {
    inner: &'a F,
    /// Keyed by path, so a path carries at most one override, the last given.
    overs: std::collections::BTreeMap<PathBuf, Override>,
}

impl<'a, F: FileSystem> OverlayFileSystem<'a, F> {
    pub(crate) fn with_all(inner: &'a F, overs: impl IntoIterator<Item = Override>) -> Self {
        Self {
            inner,
            overs: overs
                .into_iter()
                .map(|o| (o.path().to_path_buf(), o))
                .collect(),
        }
    }
}

impl<F: FileSystem> FileSystem for OverlayFileSystem<'_, F> {
    fn read_file(&self, path: &Path) -> io::Result<Vec<u8>> {
        if let Some(over) = self.overs.get(path) {
            return match over {
                Override::Replace { content, .. } => Ok(content.clone()),
                Override::Moved { from, .. } => self.inner.read_file(from),
                Override::Remove { .. } => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    path.display().to_string(),
                )),
            };
        }
        self.inner.read_file(path)
    }

    fn is_file(&self, path: &Path) -> bool {
        if let Some(over) = self.overs.get(path) {
            return match over {
                Override::Replace { .. } => true,
                Override::Moved { from, .. } => self.inner.is_file(from),
                Override::Remove { .. } => false,
            };
        }
        self.inner.is_file(path)
    }

    /// Delegated whole: an override never lands inside an identity directory, because the write
    /// guard rejects such a path before a preview is staged, so no override can add or remove one.
    fn is_dir(&self, path: &Path) -> bool {
        self.inner.is_dir(path)
    }

    /// The overridden path reports its override's size (a `Replace`'s content
    /// length, a `Moved`'s source size, or absent for a `Remove`); every other path delegates to the
    /// inner filesystem's cheap stat rather than the read-based default, so a
    /// preview honours the read cap exactly as a real build does.
    fn file_len(&self, path: &Path) -> io::Result<u64> {
        if let Some(over) = self.overs.get(path) {
            return match over {
                Override::Replace { content, .. } => Ok(content.len() as u64),
                Override::Moved { from, .. } => self.inner.file_len(from),
                Override::Remove { .. } => Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    path.display().to_string(),
                )),
            };
        }
        self.inner.file_len(path)
    }

    fn walk_files(&self, root: &Path, filter: &WalkFilter) -> io::Result<Walk> {
        let mut walk = self.inner.walk_files(root, filter)?;
        // One set over the walk, so a folder preview's many overrides cost a
        // lookup each, not a scan of the whole file list. Overrides are keyed by
        // path, so no path is both added and removed, and batching the removals
        // into one pass leaves the same files in the same order.
        let mut present: std::collections::HashSet<PathBuf> = walk.files.iter().cloned().collect();
        let mut removed: std::collections::HashSet<&Path> = std::collections::HashSet::new();
        for over in self.overs.values() {
            match over {
                // A write ADD: a new in-scope path the inner walk did not yield
                // joins the file set. An overwrite or an edit is already present,
                // so the guard leaves it untouched. The hard-floor exclusion is
                // upstream: the mutation path rejects an `.arsumbris` target
                // before a preview ever reaches this overlay.
                Override::Replace { path, .. } | Override::Moved { path, .. } => {
                    if path.starts_with(root)
                        && filter.keep_file(path)
                        && present.insert(path.clone())
                    {
                        walk.files.push(path.clone());
                    }
                }
                // A delete: the removed path leaves the file set.
                Override::Remove { path } => {
                    removed.insert(path);
                }
            }
        }
        if !removed.is_empty() {
            walk.files.retain(|p| !removed.contains(p.as_path()));
        }
        Ok(walk)
    }

    fn walk_scope_boundaries(
        &self,
        root: &Path,
        filter: &WalkFilter,
    ) -> io::Result<(ScopeBoundaries, Vec<WalkError>)> {
        // A content override does not move the scope boundaries the ignore
        // patterns decide, so the inner view is faithful. Neither does a rename,
        // the preview refuses a move of an engine-schema file upstream.
        self.inner.walk_scope_boundaries(root, filter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tb_parser::MemoryFileSystem;

    fn base() -> MemoryFileSystem {
        let mut fs = MemoryFileSystem::new();
        fs.insert("/v/a.md", b"aaa".to_vec());
        fs.insert("/v/b.md", b"bbb".to_vec());
        fs
    }

    #[test]
    fn replace_overrides_read_and_is_file() {
        let inner = base();
        let over = Override::Replace {
            path: PathBuf::from("/v/a.md"),
            content: b"NEW".to_vec(),
        };
        let fs = OverlayFileSystem::with_all(&inner, [over]);
        // The overridden path reads the would-be bytes; others fall through.
        assert_eq!(fs.read_file(Path::new("/v/a.md")).unwrap(), b"NEW");
        assert_eq!(fs.read_file(Path::new("/v/b.md")).unwrap(), b"bbb");
        assert!(fs.is_file(Path::new("/v/a.md")));
    }

    #[test]
    fn replace_of_a_new_path_is_readable_and_a_file() {
        let inner = base();
        let over = Override::Replace {
            path: PathBuf::from("/v/c.md"),
            content: b"CCC".to_vec(),
        };
        let fs = OverlayFileSystem::with_all(&inner, [over]);
        assert_eq!(fs.read_file(Path::new("/v/c.md")).unwrap(), b"CCC");
        assert!(fs.is_file(Path::new("/v/c.md")));
    }

    #[test]
    fn remove_hides_the_path_from_read_and_is_file() {
        let inner = base();
        let over = Override::Remove {
            path: PathBuf::from("/v/a.md"),
        };
        let fs = OverlayFileSystem::with_all(&inner, [over]);
        assert_eq!(
            fs.read_file(Path::new("/v/a.md")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert!(!fs.is_file(Path::new("/v/a.md")));
        // A sibling is untouched.
        assert!(fs.is_file(Path::new("/v/b.md")));
    }

    #[test]
    fn walk_injects_an_add_and_passes_an_edit_through() {
        let inner = base();
        let root = Path::new("/v");
        let filter = WalkFilter::default_excludes(root);
        // A brand-new in-scope path joins the walk.
        let add = OverlayFileSystem::with_all(
            &inner,
            [Override::Replace {
                path: PathBuf::from("/v/c.md"),
                content: b"CCC".to_vec(),
            }],
        );
        let files = add.walk_files(root, &filter).unwrap().files;
        assert!(files.iter().any(|p| p == Path::new("/v/c.md")));
        assert_eq!(files.len(), 3);
        // An edit of an existing path does not duplicate it.
        let edit = OverlayFileSystem::with_all(
            &inner,
            [Override::Replace {
                path: PathBuf::from("/v/a.md"),
                content: b"NEW".to_vec(),
            }],
        );
        let files = edit.walk_files(root, &filter).unwrap().files;
        assert_eq!(
            files.iter().filter(|p| *p == Path::new("/v/a.md")).count(),
            1
        );
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn a_move_overlays_its_source_removal_and_its_destination_add() {
        let inner = base();
        let root = Path::new("/v");
        let filter = WalkFilter::default_excludes(root);
        let fs = OverlayFileSystem::with_all(
            &inner,
            [
                Override::Remove {
                    path: PathBuf::from("/v/a.md"),
                },
                Override::Replace {
                    path: PathBuf::from("/v/sub/a.md"),
                    content: b"aaa".to_vec(),
                },
                Override::Replace {
                    path: PathBuf::from("/v/b.md"),
                    content: b"B2".to_vec(),
                },
            ],
        );
        assert!(!fs.is_file(Path::new("/v/a.md")));
        assert_eq!(fs.read_file(Path::new("/v/sub/a.md")).unwrap(), b"aaa");
        assert_eq!(fs.read_file(Path::new("/v/b.md")).unwrap(), b"B2");
        let mut files = fs.walk_files(root, &filter).unwrap().files;
        files.sort();
        assert_eq!(
            files,
            vec![PathBuf::from("/v/b.md"), PathBuf::from("/v/sub/a.md")]
        );
    }

    #[test]
    fn a_moved_path_reads_its_source_while_the_source_reads_absent() {
        let inner = base();
        let root = Path::new("/v");
        let filter = WalkFilter::default_excludes(root);
        let fs = OverlayFileSystem::with_all(
            &inner,
            [
                Override::Remove {
                    path: PathBuf::from("/v/a.md"),
                },
                Override::Moved {
                    path: PathBuf::from("/v/sub/a.md"),
                    from: PathBuf::from("/v/a.md"),
                },
            ],
        );
        // The destination serves the source's bytes, size, and presence.
        assert_eq!(fs.read_file(Path::new("/v/sub/a.md")).unwrap(), b"aaa");
        assert_eq!(fs.file_len(Path::new("/v/sub/a.md")).unwrap(), 3);
        assert!(fs.is_file(Path::new("/v/sub/a.md")));
        // The source itself is gone.
        assert!(!fs.is_file(Path::new("/v/a.md")));
        let mut files = fs.walk_files(root, &filter).unwrap().files;
        files.sort();
        assert_eq!(
            files,
            vec![PathBuf::from("/v/b.md"), PathBuf::from("/v/sub/a.md")]
        );
    }

    #[test]
    fn walk_drops_a_removed_path() {
        let inner = base();
        let root = Path::new("/v");
        let filter = WalkFilter::default_excludes(root);
        let fs = OverlayFileSystem::with_all(
            &inner,
            [Override::Remove {
                path: PathBuf::from("/v/a.md"),
            }],
        );
        let files = fs.walk_files(root, &filter).unwrap().files;
        assert!(!files.iter().any(|p| p == Path::new("/v/a.md")));
        assert!(files.iter().any(|p| p == Path::new("/v/b.md")));
        assert_eq!(files.len(), 1);
    }
}
