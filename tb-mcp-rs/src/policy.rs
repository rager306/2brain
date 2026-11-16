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
pub const MUTATION_TOOLS: [&str; 16] = [
    "tb_write_file",
    "tb_edit_file",
    "tb_delete_file",
    "tb_batch",
    "tb_assign_block_id",
    "tb_rename_block_id",
    "tb_rename",
    "tb_rename_type",
    "tb_add_dep",
    "tb_remove_dep",
    "tb_inline",
    "tb_promote",
    "tb_create_file",
    "tb_add_workspace_member",
    "tb_remove_workspace_member",
    "tb_set_workspace_member_role",
];

/// Tools that change what the workspace contains, not just a file in it.
///
/// They author `.arsumbris/repo.yaml`, so they are a second grant on top of
/// `allow_mutations`: a launch may trust writes to notes and still refuse to
/// reshape the workspace's dependency graph.
pub const COMPOSITION_TOOLS: [&str; 2] = ["tb_add_dep", "tb_remove_dep"];

/// Tools that change which members the workspace serves. A third grant on top of
/// mutations: reshaping the workspace is a different trust decision from editing
/// a note, and a mistake here changes what every later read sees.
pub const WORKSPACE_TOOLS: [&str; 3] = [
    "tb_add_workspace_member",
    "tb_remove_workspace_member",
    "tb_set_workspace_member_role",
];

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
    /// Whether composition tools (`tb_add_dep`, `tb_remove_dep`) exist. They
    /// change the workspace's dependency graph, so they are a separate grant.
    #[serde(default)]
    pub allow_composition: bool,
    /// Whether workspace-shape tools exist (`tb_add_workspace_member`,
    /// `tb_remove_workspace_member`, `tb_set_workspace_member_role`). They change
    /// which members are served, so they are a third grant.
    #[serde(default)]
    pub allow_workspace_shape: bool,
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
            allow_workspace_shape: false,
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
        if policy.allow_workspace_shape && !policy.allow_mutations {
            return Err("policy: 'allow_workspace_shape' requires 'allow_mutations'".into());
        }
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
    /// This exact path was read through `tb_content`.
    PriorRead,
    /// The recorded hash still matches the caller's expectation.
    Freshness,
    /// The file does not exist yet, which is what makes a create a create.
    Absence,
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
            Self::Absence => "absence",
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

/// The gate order for a create: the same shape as a mutation, with absence in
/// place of the read-then-hash pair. A create has no prior text to compare
/// against, so the guarantee is the opposite one: the path must be free.
pub const CREATE_STAGES: [Stage; 6] = [
    Stage::Arguments,
    Stage::PathScope,
    Stage::MemberGrant,
    Stage::Absence,
    Stage::Editability,
    Stage::Capacity,
];

/// A create's refusal when the path is taken: one string, so the local pre-check
/// and the engine's mid-write conflict — the same gate answering a few
/// milliseconds apart — say the same thing to the client.
pub const CREATE_ALREADY_EXISTS: &str = "file already exists; use tb_write_file to change it";

/// At most this many paths are named per member in a cascade refusal.
const CASCADE_PATHS_SHOWN: usize = 2;

/// At most this many members are listed before the rest are counted.
const CASCADE_MEMBERS_SHOWN: usize = 4;

/// The refusal when a `rename_type` cascade would write members this launch does
/// not allow: the definition's owner is permitted, its referrers are not.
///
/// `outside` is the member and the paths the rename would rewrite there, already
/// member-sorted by the caller. The message names the members, because the fix is
/// a policy edit rather than a guess, and says plainly that the grant covers the
/// cascade too — the tool checks the owner's member first, so a caller could
/// otherwise read the refusal as a contradiction.
pub(crate) fn rename_type_cascade_denial(
    type_name: &str,
    owner: &str,
    outside: &[(String, Vec<String>)],
) -> Denial {
    let mut listed: Vec<String> = outside
        .iter()
        .take(CASCADE_MEMBERS_SHOWN)
        .map(|(member, paths)| {
            let mut shown: Vec<&str> = paths
                .iter()
                .take(CASCADE_PATHS_SHOWN)
                .map(String::as_str)
                .collect();
            if paths.len() > CASCADE_PATHS_SHOWN {
                shown.push("…");
            }
            format!("'{member}' ({})", shown.join(", "))
        })
        .collect();
    if outside.len() > CASCADE_MEMBERS_SHOWN {
        listed.push(format!(
            "and {} more member(s)",
            outside.len() - CASCADE_MEMBERS_SHOWN
        ));
    }
    Denial::new(
        Stage::MemberGrant,
        format!(
            "the type '{type_name}' is defined in member '{owner}', which this launch allows, but \
             renaming it would also rewrite files in {} member(s) outside the grant: {}. \
             A rename_type carries claims, slot shapes, meta and wikilinks to every referrer, so \
             the grant must cover the members it rewrites, not only the one that defines the type. \
             Add them to the policy's 'members', or rename the type where every referrer's member \
             is allowed",
            outside.len(),
            listed.join(", "),
        ),
    )
}

/// Appends policy decisions. Cloning shares the same sink and the same chain.
#[derive(Debug, Clone)]
pub struct Audit {
    inner: Option<std::sync::Arc<Mutex<AuditSink>>>,
    chain: std::sync::Arc<Mutex<Chain>>,
}

/// The tail of the audit hash chain: the last sequence number and hash.
///
/// Each record carries both, so a reader can prove that no line was removed or
/// edited in the middle: editing one line breaks its own hash, and removing one
/// breaks the `prev` link of the next.
#[derive(Debug)]
struct Chain {
    seq: u64,
    prev: String,
}

impl Chain {
    /// The `prev` of the first record: sixty-four zeros, so no record is
    /// indistinguishable from a head.
    const GENESIS: &'static str =
        "0000000000000000000000000000000000000000000000000000000000000000";
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
        Self {
            inner,
            chain: std::sync::Arc::new(Mutex::new(Chain {
                seq: 0,
                prev: Chain::GENESIS.to_owned(),
            })),
        }
    }

    /// Record one decision. Failures are reported on stderr but never abort the
    /// operation: the decision itself has already been made.
    pub fn record(&self, principal: &str, tool: &str, member: &str, outcome: &str) {
        let Some(inner) = self.inner.as_ref() else {
            return;
        };
        let mut sink = inner.lock().expect("audit lock poisoned");
        let mut chain = self.chain.lock().expect("audit chain lock poisoned");
        let seq = chain.seq + 1;
        let prev = chain.prev.clone();
        let mut record = serde_json::json!({
            "event": "mcp_policy",
            "principal": principal,
            "tool": tool,
            "member": member,
            "outcome": outcome,
            "timestamp_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0),
            "seq": seq,
            "prev": prev,
        });
        // serde_json orders object keys, so the preimage is the record minus the
        // hash, serialized again: a reader can recompute it from the line alone.
        let hash = digest(&record.to_string());
        record["hash"] = serde_json::Value::String(hash.clone());
        let line = record.to_string();
        match &mut *sink {
            AuditSink::Stderr => eprintln!("{line}"),
            AuditSink::File(path) => match fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&*path)
            {
                Ok(mut file) => {
                    if writeln!(file, "{line}").is_err() {
                        eprintln!("tb-mcp-rs: audit write failed for {}", path.display());
                        return;
                    }
                }
                Err(error) => {
                    eprintln!("tb-mcp-rs: audit open failed: {error}");
                    return;
                }
            },
        }
        chain.seq = seq;
        chain.prev = hash;
    }
}

/// The SHA-256 of one record, hex-encoded: the chain's link.
fn digest(preimage: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(preimage.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The first gate that refused a mutation.
#[derive(Debug)]
pub(crate) struct Denial {
    pub(crate) stage: Stage,
    pub(crate) message: String,
}

impl Denial {
    pub(crate) fn new(stage: Stage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }

    /// The refusal as the caller sees it.
    ///
    /// The stage code (`policy/<stage>`) is the machine-readable half — the audit log
    /// records `denied:<stage>` from the same value — and the message is the human
    /// half. Each message already names the step that clears it; a separate remedy
    /// table would only repeat them (measured: it broke a test that pins the text).
    pub(crate) fn text(&self) -> String {
        format!("policy/{}: {}", self.stage.name(), self.message)
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
    fn a_denial_names_its_stage_before_its_reason() {
        let denial = Denial::new(Stage::PriorRead, "read this file first");
        assert_eq!(
            denial.text(),
            "policy/prior_read: read this file first",
            "the stage code is the machine-readable half and must come first"
        );
        // The audit log records the same value, so a client can correlate the two.
        assert_eq!(denial.stage.name(), "prior_read");
        let freshness = Denial::new(Stage::Freshness, "hash moved");
        assert!(freshness.text().starts_with("policy/freshness: "));
        assert_ne!(
            denial.text(),
            freshness.text(),
            "two stages must not collapse into one code"
        );
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

    #[test]
    fn audit_records_form_a_hash_chain() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        let audit = Audit::new(&AuditTarget::File(path.clone()));
        audit.record("me", "tb_write_file", "fixture", "denied:prior_read");
        audit.record("me", "tb_create_file", "fixture", "applied");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["seq"], 1);
        assert_eq!(lines[0]["prev"], Chain::GENESIS);
        assert_eq!(lines[1]["seq"], 2);
        assert_eq!(lines[1]["prev"], lines[0]["hash"]);
        for line in &lines {
            let mut fields = line.as_object().unwrap().clone();
            let stored = fields.remove("hash").unwrap();
            let stored = stored.as_str().unwrap();
            assert_eq!(
                digest(&serde_json::Value::Object(fields).to_string()),
                stored
            );
        }
        // Editing a field breaks that record's own hash, which is the point.
        let mut tampered = lines[1].as_object().unwrap().clone();
        tampered.insert("outcome".into(), "rejected".into());
        let stored = tampered.remove("hash").unwrap();
        assert_ne!(
            digest(&serde_json::Value::Object(tampered).to_string()),
            stored.as_str().unwrap()
        );
    }
}
