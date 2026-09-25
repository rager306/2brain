//! Daemon-less member resolution: which of an entry's declared members mount,
//! where, and through which tier, answered without booting an engine.
//!
//! A launcher checks a workspace before it starts the daemon. It needs the
//! engine's own answer, the resolution order a build runs (co-present sibling,
//! then per-user registry, then package cache), not a re-derivation from the
//! registry alone. This runs exactly the build's composition,
//! [`crate::build::compose`], then its member-outcome decision,
//! [`crate::build::member_mount`], so the two cannot disagree on what mounts.
//!
//! Reads only. No write, no registry change, no fetch: the cache is consulted by
//! locked sha, and a dependency not yet fetched is reported, never fetched.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use au_diagnostics::Diagnostic;
use au_parser::FileSystem;
use serde::Serialize;

use crate::build::{compose, member_mount, BuildError, Composition, MemberMount};
use crate::repo::{MemberRole, RepoName, ResolveNote, UserRegistry};

/// An entry's member resolution, the result of [`resolve_members`].
#[derive(Debug, Serialize)]
pub struct MemberResolution {
    /// The entry directory the members were resolved for.
    pub entry: PathBuf,
    /// `false` when the member list cannot be the whole answer: a locked
    /// transitive dependency whose snapshot is absent from the package cache is
    /// left out of it. Running `resolve` completes it. The causes are in
    /// `incomplete`.
    pub complete: bool,
    /// Why the answer is incomplete, each a diagnostic, empty when `complete`.
    pub incomplete: Vec<Diagnostic>,
    /// Every declared member, name-sorted.
    pub members: Vec<MemberResolutionEntry>,
    /// The workspace-level diagnostics the build emits for this composition,
    /// the member-outcome ones included. The authoritative list; each member's
    /// `code` is a per-member key into it.
    pub diagnostics: Vec<Diagnostic>,
}

/// One declared member and how it resolved.
#[derive(Debug, Serialize)]
pub struct MemberResolutionEntry {
    /// The member's declared name.
    pub name: String,
    /// `entry`, `edit`, `discover`, or `dep`. A disabled member carries its
    /// declared `edit` / `discover` role.
    pub role: &'static str,
    /// Which tier located the member: `entry`, `sibling`, `registry`, `cache`,
    /// `unmounted`, or `disabled` for a member the workspace switches off on
    /// purpose. Only `unmounted` wants the user to locate it.
    pub tier: &'static str,
    /// The resolved root, absent when no tier located it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// The member-outcome diagnostic code, absent for a cleanly mounted member.
    /// For an unmounted member, the cause the build names: the role-keyed
    /// unmounted code, or the resolution clash that left it unmounted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Resolve an entry's members the way a build does, without building.
///
/// Refuses a non-repo entry with the build's own [`BuildError`]. `registry` and
/// `cache_root` are the same inputs the engine passes its build, so a caller
/// reproduces a daemon's answer by passing what the daemon would. A `None`
/// `cache_root` consults no lock, exactly as the build does.
pub fn resolve_members(
    entry: &Path,
    fs: &impl FileSystem,
    registry: &UserRegistry,
    cache_root: Option<&Path>,
) -> Result<MemberResolution, BuildError> {
    let Composition {
        entry_ws: ws,
        repos,
        member_walk_errors,
        dep_notes,
        diagnostics: compose_diags,
        ..
    } = compose(entry, fs, registry, cache_root)?;

    let mut diagnostics = compose_diags;
    diagnostics.extend(crate::build::consistency_diagnostics(
        &repos,
        std::slice::from_ref(&ws),
        &member_walk_errors,
        &dep_notes,
        registry,
        cache_root,
    ));

    // Every declared member: the resolved set plus any disabled declaration,
    // which never enters resolution.
    let mut names: BTreeSet<RepoName> = ws.members.iter().map(|m| m.name.clone()).collect();
    names.extend(ws.member_roles.keys().cloned());
    for name in &ws.disabled {
        if ws.edit.contains(name) || ws.discover.contains(name) {
            names.insert(name.clone());
        }
    }

    let members = names
        .into_iter()
        .map(|name| match ws.member_roles.get(&name) {
            Some(role) => {
                let mount = member_mount(&ws, &name, &repos, &member_walk_errors);
                let (tier, code) = outcome(&ws, &name, *role, &mount, cache_root);
                MemberResolutionEntry {
                    name: name.as_str().to_string(),
                    role: role.as_str(),
                    tier,
                    path: ws.member_paths.get(&name).cloned(),
                    code,
                }
            }
            // Not resolved: a disabled declaration, switched off on purpose.
            None => MemberResolutionEntry {
                name: name.as_str().to_string(),
                role: if ws.edit.contains(&name) {
                    MemberRole::Edit.as_str()
                } else {
                    MemberRole::Discover.as_str()
                },
                tier: "disabled",
                path: None,
                code: None,
            },
        })
        .collect();

    // A locked transitive dependency the cache does not hold is left out of the
    // member set entirely, so the list alone cannot show it.
    let incomplete: Vec<Diagnostic> = diagnostics
        .iter()
        .filter(|d| d.code == crate::repo::DEPENDENCY_CACHE_MISS)
        .cloned()
        .collect();

    Ok(MemberResolution {
        entry: entry.to_path_buf(),
        complete: incomplete.is_empty(),
        incomplete,
        members,
        diagnostics,
    })
}

/// A resolved member's tier and outcome code, the same role-keyed decision the
/// build's member-outcome diagnostics take.
fn outcome(
    ws: &crate::repo::Workspace,
    name: &RepoName,
    role: MemberRole,
    mount: &MemberMount<'_>,
    cache_root: Option<&Path>,
) -> (&'static str, Option<String>) {
    match mount {
        MemberMount::Mounted(root) => {
            // Every mount goes through `Workspace::mount`, which records the tier
            // with the path, so a mounted member without one is a broken invariant.
            let tier = ws
                .tier_of(name)
                .expect("a mounted member carries its tier, see Workspace::mount")
                .as_str();
            let read_only = cache_root.is_some_and(|c| root.starts_with(c));
            let code = (role.editable() && read_only)
                .then(|| crate::repo::EDIT_MEMBER_READ_ONLY.as_str().to_string());
            (tier, code)
        }
        MemberMount::ReservedRoot => (
            "unmounted",
            Some(crate::repo::MEMBER_AT_RESERVED_ROOT.as_str().to_string()),
        ),
        MemberMount::Unwalkable(_) => (
            "unmounted",
            Some(
                crate::repo::WORKSPACE_MEMBER_UNWALKABLE
                    .as_str()
                    .to_string(),
            ),
        ),
        MemberMount::Unmounted => ("unmounted", Some(unmounted_code(ws, name, role))),
    }
}

/// Why an unmounted member did not mount: a resolution clash its notes record,
/// else the role-keyed unmounted code.
fn unmounted_code(ws: &crate::repo::Workspace, name: &RepoName, role: MemberRole) -> String {
    for (n, note) in &ws.member_notes {
        if n != name {
            continue;
        }
        let code = match note {
            ResolveNote::DuplicateSibling { .. } => crate::repo::DUPLICATE_REPO_NAME,
            ResolveNote::IdentityConflict { .. } => crate::repo::DEPENDENCY_IDENTITY_CONFLICT,
            ResolveNote::MalformedRepoYaml(d) => d.code.clone(),
            ResolveNote::PathOverridden { .. } | ResolveNote::ReservedRoot => continue,
        };
        return code.as_str().to_string();
    }
    let code = match role {
        MemberRole::Entry | MemberRole::Edit => crate::repo::EDIT_MEMBER_UNMOUNTED,
        MemberRole::Discover => crate::repo::DISCOVER_MEMBER_UNMOUNTED,
        MemberRole::Dep => crate::repo::PEER_UNMOUNTED,
    };
    code.as_str().to_string()
}
