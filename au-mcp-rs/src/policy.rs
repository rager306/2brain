//! Launch policy: what this server process may do.
//!
//! A stdio MCP server is launched by its client, so the only trustworthy
//! boundary is the process itself. This module makes that boundary explicit and
//! fail-closed: without an explicit grant the server is read-only, and every
//! mutation decision is recorded.
//!
//! Identity is **not** authenticated. The principal name comes from the launch
//! arguments or the client's `initialize`, so it labels audit records; it does
//! not prove who called.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::Deserialize;

/// Tools that change state. Everything else is a read.
pub const MUTATION_TOOLS: [&str; 11] = [
    "au_write_file",
    "au_edit_file",
    "au_delete_file",
    "au_assign_block_id",
    "au_rename_block_id",
    "au_rename",
    "au_rename_type",
    "au_add_dep",
    "au_remove_dep",
    "au_inline",
    "au_promote",
];

/// Tools that change what the workspace contains, not just a file in it.
///
/// They author `.arsumbris/repo.yaml`, so they are a second grant on top of
/// `allow_mutations`: a launch may trust writes to notes and still refuse to
/// reshape the workspace's dependency graph.
pub const COMPOSITION_TOOLS: [&str; 2] = ["au_add_dep", "au_remove_dep"];

/// Where mutation decisions are written.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditTarget {
    /// One JSON object per line on standard error.
    Stderr,
    /// Append the same lines to this file.
    File(PathBuf),
    /// Skip the audit trail entirely. Only valid with mutations denied.
    Disabled,
}

impl AuditTarget {
    /// A one-line description for `--info`.
    pub fn describe(&self) -> String {
        match self {
            Self::Stderr => "stderr".to_owned(),
            Self::File(path) => format!("file {}", path.display()),
            Self::Disabled => "disabled".to_owned(),
        }
    }
}

/// The parsed launch policy.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Whether the mutating tools exist at all. Absent means denied.
    #[serde(default)]
    pub allow_mutations: bool,
    /// Whether composition tools (`au_add_dep`, `au_remove_dep`) exist. They
    /// change the workspace's dependency graph, so they are a separate grant.
    #[serde(default)]
    pub allow_composition: bool,
    /// Members a mutation may target. Absent means every editable member.
    #[serde(default)]
    pub members: Option<Vec<String>>,
    /// Where decisions are recorded.
    #[serde(default = "default_audit")]
    pub audit: AuditTarget,
}

fn default_audit() -> AuditTarget {
    AuditTarget::Stderr
}

impl Policy {
    /// The default: reads only.
    pub fn read_only() -> Self {
        Self {
            allow_mutations: false,
            allow_composition: false,
            members: None,
            audit: AuditTarget::Stderr,
        }
    }

    /// Load a policy file. A malformed or unreadable file is an error, never a
    /// silent fallback to a permissive default.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path)
            .map_err(|error| format!("reading policy {}: {error}", path.display()))?;
        let policy: Self = serde_json::from_str(&text)
            .map_err(|error| format!("parsing policy {}: {error}", path.display()))?;
        if policy.allow_composition && !policy.allow_mutations {
            return Err("policy: 'allow_composition' requires 'allow_mutations'".into());
        }
        if policy.allow_mutations && policy.audit == AuditTarget::Disabled {
            return Err(
                "policy: 'audit: disabled' is only valid while mutations are denied".into(),
            );
        }
        Ok(policy)
    }

    /// Whether a mutation may target this member.
    pub fn permits_member(&self, member: &str) -> bool {
        match &self.members {
            Some(members) => members.iter().any(|allowed| allowed == member),
            None => true,
        }
    }
}

/// The ordered gate a mutation passes before the engine is contacted.
///
/// The order is the contract: cheap, local refusals come first, and no later
/// stage runs once an earlier one denies. The mutation slot is taken last, so a
/// permit is never held while an engine read settles a file's editability. A denial always names its stage, both
/// in the tool error and in the audit line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// The arguments themselves are usable.
    Arguments,
    /// The path cannot escape the member root.
    PathScope,
    /// The launch policy allows mutations in this member.
    MemberGrant,
    /// This exact path was read through `au_content`.
    PriorRead,
    /// The recorded hash still matches the caller's expectation.
    Freshness,
    /// A mutation slot is free.
    Capacity,
    /// The engine will author this member.
    Editability,
}

impl Stage {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Arguments => "arguments",
            Self::PathScope => "path_scope",
            Self::MemberGrant => "member_grant",
            Self::PriorRead => "prior_read",
            Self::Freshness => "freshness",
            Self::Capacity => "capacity",
            Self::Editability => "editability",
        }
    }
}

/// The gate order, in the order it runs.
pub const MUTATION_STAGES: [Stage; 7] = [
    Stage::Arguments,
    Stage::PathScope,
    Stage::MemberGrant,
    Stage::PriorRead,
    Stage::Freshness,
    Stage::Editability,
    Stage::Capacity,
];

/// Appends policy decisions. Cloning shares the same sink.
#[derive(Debug, Clone)]
pub struct Audit {
    inner: Option<std::sync::Arc<Mutex<AuditSink>>>,
}

#[derive(Debug)]
enum AuditSink {
    Stderr,
    File(PathBuf),
}

impl Audit {
    pub fn new(target: &AuditTarget) -> Self {
        let inner = match target {
            AuditTarget::Stderr => Some(std::sync::Arc::new(Mutex::new(AuditSink::Stderr))),
            AuditTarget::File(path) => Some(std::sync::Arc::new(Mutex::new(AuditSink::File(
                path.clone(),
            )))),
            AuditTarget::Disabled => None,
        };
        Self { inner }
    }

    /// Record one decision. Failures are reported on stderr but never abort the
    /// operation: the decision itself has already been made.
    pub fn record(&self, principal: &str, tool: &str, member: &str, outcome: &str) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let line = serde_json::json!({
            "event": "mcp_policy",
            "principal": principal,
            "tool": tool,
            "member": member,
            "outcome": outcome,
            "timestamp_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
        })
        .to_string();
        let mut sink = inner.lock().expect("audit lock poisoned");
        match &mut *sink {
            AuditSink::Stderr => eprintln!("{line}"),
            AuditSink::File(path) => match fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&*path)
            {
                Ok(mut file) => {
                    if writeln!(file, "{line}").is_err() {
                        eprintln!("au-mcp-rs: audit write failed for {}", path.display());
                    }
                }
                Err(error) => eprintln!("au-mcp-rs: audit open failed: {error}"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_denies_mutations_and_permits_no_member() {
        let policy = Policy::read_only();
        assert!(!policy.allow_mutations);
        assert!(policy.permits_member("anything"));
    }

    #[test]
    fn member_allowlist_is_exact() {
        let policy: Policy =
            serde_json::from_str(r#"{"allow_mutations":true,"members":["edit-me"]}"#).unwrap();
        assert!(policy.permits_member("edit-me"));
        assert!(!policy.permits_member("edit-me-too"));
        assert!(!policy.permits_member("dep"));
    }

    #[test]
    fn audit_cannot_be_disabled_while_mutations_are_allowed() {
        let dir = std::env::temp_dir().join(format!("au-policy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.json");
        std::fs::write(&path, r#"{"allow_mutations":true,"audit":"disabled"}"#).unwrap();
        let error = Policy::load(&path).unwrap_err();
        assert!(error.contains("audit"), "{error}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_gate_order_is_explicit() {
        assert_eq!(
            MUTATION_STAGES.map(Stage::name),
            [
                "arguments",
                "path_scope",
                "member_grant",
                "prior_read",
                "freshness",
                "editability",
                "capacity",
            ]
        );
    }

    #[test]
    fn audit_targets_parse() {
        let file: Policy = serde_json::from_str(r#"{"audit":{"file":"/tmp/a.jsonl"}}"#).unwrap();
        assert_eq!(file.audit, AuditTarget::File(PathBuf::from("/tmp/a.jsonl")));
        let stderr: Policy = serde_json::from_str("{}").unwrap();
        assert_eq!(stderr.audit, AuditTarget::Stderr);
        assert!(!stderr.allow_mutations);
    }

    #[test]
    fn malformed_policy_is_an_error() {
        let dir = std::env::temp_dir().join(format!("au-policy-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(Policy::load(&path).is_err());
        std::fs::write(&path, r#"{"allow_mutations":true,"unknown":1}"#).unwrap();
        assert!(Policy::load(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
