//! One capability class per advertised tool.
//!
//! The class answers "what kind of capability is this" for discovery and for tests. It
//! is deliberately **not** the grant model: grants are nested (`workspace` and
//! `composition` are subsets of `mutations`, measured 16 / 2 / 3), while a capability is
//! single-valued so that a filter can be total and unambiguous.
//!
//! The table is the single place a tool gets classified. A tool that is advertised
//! without a row is a defect the tests catch, not a default.

use serde::{Deserialize, Serialize};

/// What kind of capability a tool offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Capability {
    /// Reads engine or repository state; advertised without any grant.
    Read,
    /// Changes workspace content; requires the mutations grant (and, for composition
    /// and workspace shape, one more grant on top).
    Mutate,
    /// Derives an answer from the tool catalogue itself, without contacting the engine.
    Discover,
    /// Subscribes to change notifications without reading engine state.
    Watch,
}

/// Every advertised tool with its one class. Order is by name so a reader can find a
/// tool without a search; the tests assert the set matches the router exactly.
pub(crate) const CAPABILITIES: [(&str, Capability); 52] = [
    ("au_add_dep", Capability::Mutate),
    ("au_add_workspace_member", Capability::Mutate),
    ("au_anchors", Capability::Read),
    ("au_assign_block_id", Capability::Mutate),
    ("au_batch", Capability::Mutate),
    ("au_block_ids", Capability::Read),
    ("au_candidates", Capability::Read),
    ("au_children", Capability::Read),
    ("au_content", Capability::Read),
    ("au_create_file", Capability::Mutate),
    ("au_delete_file", Capability::Mutate),
    ("au_describe_tool", Capability::Discover),
    ("au_diagnostics", Capability::Read),
    ("au_edit_file", Capability::Mutate),
    ("au_files", Capability::Read),
    ("au_follow", Capability::Read),
    ("au_frontmatter", Capability::Read),
    ("au_graph_health", Capability::Read),
    ("au_grep", Capability::Read),
    ("au_hubs", Capability::Read),
    ("au_imports", Capability::Read),
    ("au_inline", Capability::Mutate),
    ("au_instances", Capability::Read),
    ("au_instances_of", Capability::Read),
    ("au_members", Capability::Read),
    ("au_neighborhood", Capability::Read),
    ("au_overview", Capability::Read),
    ("au_preview_mutation", Capability::Read),
    ("au_promote", Capability::Mutate),
    ("au_reconcile", Capability::Read),
    ("au_references", Capability::Read),
    ("au_remove_dep", Capability::Mutate),
    ("au_remove_workspace_member", Capability::Mutate),
    ("au_rename", Capability::Mutate),
    ("au_rename_block_id", Capability::Mutate),
    ("au_rename_type", Capability::Mutate),
    ("au_resolve_member", Capability::Read),
    ("au_resolved", Capability::Read),
    ("au_rollback_preview", Capability::Read),
    ("au_search", Capability::Read),
    ("au_search_tools", Capability::Discover),
    ("au_semantic_tokens", Capability::Read),
    ("au_set_workspace_member_role", Capability::Mutate),
    ("au_subtypes", Capability::Read),
    ("au_top_level_dirs", Capability::Read),
    ("au_type", Capability::Read),
    ("au_type_tree", Capability::Read),
    ("au_typed", Capability::Read),
    ("au_types", Capability::Read),
    ("au_validate_value", Capability::Read),
    ("au_watch", Capability::Watch),
    ("au_write_file", Capability::Mutate),
];

/// The class of one tool, or `None` when the table does not cover it.
pub(crate) fn of(tool: &str) -> Option<Capability> {
    CAPABILITIES
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, capability)| *capability)
}
