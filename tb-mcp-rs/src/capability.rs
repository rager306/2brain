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
    ("tb_add_dep", Capability::Mutate),
    ("tb_add_workspace_member", Capability::Mutate),
    ("tb_anchors", Capability::Read),
    ("tb_assign_block_id", Capability::Mutate),
    ("tb_batch", Capability::Mutate),
    ("tb_block_ids", Capability::Read),
    ("tb_candidates", Capability::Read),
    ("tb_children", Capability::Read),
    ("tb_content", Capability::Read),
    ("tb_create_file", Capability::Mutate),
    ("tb_delete_file", Capability::Mutate),
    ("tb_describe_tool", Capability::Discover),
    ("tb_diagnostics", Capability::Read),
    ("tb_edit_file", Capability::Mutate),
    ("tb_files", Capability::Read),
    ("tb_follow", Capability::Read),
    ("tb_frontmatter", Capability::Read),
    ("tb_graph_health", Capability::Read),
    ("tb_grep", Capability::Read),
    ("tb_hubs", Capability::Read),
    ("tb_imports", Capability::Read),
    ("tb_inline", Capability::Mutate),
    ("tb_instances", Capability::Read),
    ("tb_instances_of", Capability::Read),
    ("tb_members", Capability::Read),
    ("tb_neighborhood", Capability::Read),
    ("tb_overview", Capability::Read),
    ("tb_preview_mutation", Capability::Read),
    ("tb_promote", Capability::Mutate),
    ("tb_reconcile", Capability::Read),
    ("tb_references", Capability::Read),
    ("tb_remove_dep", Capability::Mutate),
    ("tb_remove_workspace_member", Capability::Mutate),
    ("tb_rename", Capability::Mutate),
    ("tb_rename_block_id", Capability::Mutate),
    ("tb_rename_type", Capability::Mutate),
    ("tb_resolve_member", Capability::Read),
    ("tb_resolved", Capability::Read),
    ("tb_rollback_preview", Capability::Read),
    ("tb_search", Capability::Read),
    ("tb_search_tools", Capability::Discover),
    ("tb_semantic_tokens", Capability::Read),
    ("tb_set_workspace_member_role", Capability::Mutate),
    ("tb_subtypes", Capability::Read),
    ("tb_top_level_dirs", Capability::Read),
    ("tb_type", Capability::Read),
    ("tb_type_tree", Capability::Read),
    ("tb_typed", Capability::Read),
    ("tb_types", Capability::Read),
    ("tb_validate_value", Capability::Read),
    ("tb_watch", Capability::Watch),
    ("tb_write_file", Capability::Mutate),
];

/// The class of one tool, or `None` when the table does not cover it.
pub(crate) fn of(tool: &str) -> Option<Capability> {
    CAPABILITIES
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, capability)| *capability)
}
