//! In-repo runtime output that git must never see.
//!
//! The engine writes ephemeral state inside a repo's working tree, under its own
//! `<repo>/.arsumbris/au-engine/` namespace: the saga's recovery markers and
//! parked pre-images in `run/`, the operation trace in `logs/trace/`. None of it
//! is ever committed, see
//! [[spec - 2brain layout - a reserved multi-tenant device root, owner-namespaced with a category sublayer]].
//!
//! The engine's own commits never pick it up, they are pathspec-scoped. A
//! human's `git add -A` would, and a parked pre-image holds the bytes of a file
//! git ignores, possibly private. So each such directory ignores itself: a
//! `.gitignore` holding `*`, written when the directory is created. It lives in
//! the engine's namespace, so no user `.gitignore` is touched, and it holds in
//! any working tree, a repo nested in a larger one included.

use std::io;
use std::path::Path;

/// The self-ignoring `.gitignore`: `*` matches every entry, itself included.
const SELF_IGNORE: &str = "# Written by au-engine: runtime output, never committed.\n*\n";

/// Create `dir` if needed, and make git ignore everything in it.
///
/// Writes `dir/.gitignore` only when absent, so an existing one is never
/// clobbered. Idempotent, so every writer into the directory calls it.
pub fn ensure_git_ignored_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let ignore = dir.join(".gitignore");
    if ignore.symlink_metadata().is_err() {
        std::fs::write(&ignore, SELF_IGNORE)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn a_human_add_all_never_stages_the_directory() {
        let repo = tempfile::tempdir().unwrap();
        let p = repo.path();
        git(p, &["init", "-q", "-b", "main"]);
        let run = p.join(".arsumbris/au-engine/run");
        ensure_git_ignored_dir(&run).unwrap();
        std::fs::write(run.join("saga.intent"), "mutation-id=m\n").unwrap();
        std::fs::create_dir_all(run.join("preimage.m")).unwrap();
        std::fs::write(run.join("preimage.m/0"), "private").unwrap();
        std::fs::write(p.join("note.md"), "n\n").unwrap();
        git(p, &["add", "-A"]);
        assert_eq!(
            git(p, &["diff", "--cached", "--name-only"]),
            "note.md",
            "only the content is staged"
        );
        assert_eq!(
            git(p, &["status", "--porcelain", "--untracked-files=all"]),
            "A  note.md"
        );
    }

    #[test]
    fn an_existing_gitignore_is_never_clobbered_and_the_call_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        ensure_git_ignored_dir(&run).unwrap();
        assert_eq!(
            std::fs::read_to_string(run.join(".gitignore")).unwrap(),
            SELF_IGNORE
        );
        std::fs::write(run.join(".gitignore"), "custom\n").unwrap();
        ensure_git_ignored_dir(&run).unwrap();
        assert_eq!(
            std::fs::read_to_string(run.join(".gitignore")).unwrap(),
            "custom\n"
        );
    }
}
