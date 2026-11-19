//! The `--help` text.
//!
//! The prose is static; the surface sentence is not. It used to read "three mutating
//! tools" while the router advertised sixteen, because a count typed into prose has no
//! relationship to the catalogue it describes. Here the numbers come from
//! `capability::CAPABILITIES`, which is the table the router is tested against, so the
//! sentence cannot drift from the surface it summarizes.

use crate::capability::{CAPABILITIES, Capability};

/// How many tools the surface carries, by capability class.
pub(crate) struct Surface {
    pub(crate) read: usize,
    pub(crate) discover: usize,
    pub(crate) watch: usize,
    pub(crate) mutate: usize,
}

impl Surface {
    /// Counted from the capability table, never from a literal.
    pub(crate) fn current() -> Self {
        let mut surface = Self {
            read: 0,
            discover: 0,
            watch: 0,
            mutate: 0,
        };
        for (_, class) in CAPABILITIES {
            match class {
                Capability::Read => surface.read += 1,
                Capability::Mutate => surface.mutate += 1,
                Capability::Discover => surface.discover += 1,
                Capability::Watch => surface.watch += 1,
            }
        }
        surface
    }

    pub(crate) fn total(&self) -> usize {
        self.read + self.discover + self.watch + self.mutate
    }

    /// Every tool a client sees without any grant.
    pub(crate) fn read_only(&self) -> usize {
        self.total() - self.mutate
    }

    /// The SURFACE sentence. The counts come from the catalogue, so the sentence cannot
    /// disagree with `--info`, which counts the same table.
    fn sentence(&self) -> String {
        format!(
            "{} tools: {} read-only ({} reads, {} discovery, {} watch) and\n    \
             {} mutating, which exist only with a grant. Run --info for the live\n    counts.",
            self.total(),
            self.read_only(),
            self.read,
            self.discover,
            self.watch,
            self.mutate,
        )
    }
}

/// The whole `--help` text, with the surface sentence built from the catalogue.
pub(crate) fn text() -> String {
    format!(
        "{}\n\nSURFACE\n    {}\n{}",
        PREFIX,
        Surface::current().sentence(),
        SUFFIX
    )
}

const PREFIX: &str = "tb-mcp-rs — MCP stdio server for the 2brain engine

USAGE
    tb-mcp-rs <engine-socket> [OPTIONS]
    tb-mcp-rs --help | --version | --info [<engine-socket>]

ARGUMENT
    <engine-socket>   Unix socket of a running `au` engine daemon,
                      usually <state root>/engine/run/<hash>.sock

OPTIONS
    --policy <file>          JSON policy: allow_mutations, members, audit
    --allow-mutations        Grant the mutating tools (off by default)
    --allow-composition      Also grant dependency edits; implies --allow-mutations
    --allow-workspace-shape  Also grant member add/remove/role; implies --allow-mutations
    --principal <name>       Label for audit records; not authentication
    --tools full|concise     Definition detail in tools/list (default concise)
    --payload both|text|structured
                             Result representation (default both)
    --info                   Print MCP and launch information, then exit
    -h, --help               Print this help, then exit
    -V, --version            Print the version, then exit";

const SUFFIX: &str = "
PROTOCOL
    Legacy: 2024-11-05, 2025-03-26, 2025-06-18, 2025-11-25 (initialize first)
    Modern: 2026-07-28 (version in _meta, no initialize)

DOCS
    docs/connecting.md   client registration and troubleshooting
    docs/tools.md        every tool and its failure modes
";
