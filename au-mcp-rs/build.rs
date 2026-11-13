//! Capture the build identity: the git commit this binary was built from, whether
//! the tree was dirty, the target triple, and the profile.
//!
//! Everything lands in compile-time environment variables, so the binary can
//! report exactly which source it came from. A tarball build without `.git`
//! degrades to `unknown` instead of failing.

use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn git(crate_dir: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(crate_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn main() {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    // Rebuild when the commit moves, so a rebuilt binary never reports a stale
    // hash. `rerun-if-changed` resolves relative paths against the package root,
    // and the repository root is one level up, so ask git for the absolute git
    // directory instead of guessing. HEAD alone does not change on a commit: the
    // branch ref and the reflog do.
    let git_dir = git(crate_dir, &["rev-parse", "--absolute-git-dir"])
        .map(PathBuf::from);
    if let Some(git_dir) = git_dir {
        for watched in ["HEAD", "index", "logs/HEAD", "packed-refs"] {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(watched).display()
            );
        }
    }
    println!("cargo:rerun-if-env-changed=AU_BUILD_COMMIT");

    let commit = env::var("AU_BUILD_COMMIT")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| git(crate_dir, &["rev-parse", "--short=12", "HEAD"]))
        .unwrap_or_else(|| "unknown".to_owned());
    // Only this crate's own files count: another repository's changes are not
    // this binary's business, and `au-engine` lives beside it.
    let dirty = match git(
        crate_dir,
        &["status", "--porcelain", "--", ".", ":(exclude)target"],
    ) {
        Some(status) => !status.is_empty(),
        None => false,
    };
    let target = env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    let profile = env::var("PROFILE").unwrap_or_else(|_| "unknown".to_owned());

    println!("cargo:rustc-env=AU_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=AU_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=AU_BUILD_TARGET={target}");
    println!("cargo:rustc-env=AU_BUILD_PROFILE={profile}");
}
