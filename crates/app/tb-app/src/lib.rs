//! Application layer: hexagonal ports (traits) and use-case orchestration.
//!
//! Depends ONLY on `crates/domain/*`. Concrete I/O adapters live in
//! `crates/adapters/*` and are wired once in `crates/bin/two-brain`.

use tb_contracts::{CasVerified, EvidenceReceipt, Generation, Locator, WriteIntent};

/// Driven port for workspace filesystem and Git access.
pub trait WorkspaceStore: Send + Sync {
    type Error: std::error::Error + Send + Sync + 'static;

    fn read_exact(&self, locator: &Locator) -> Result<(Vec<u8>, EvidenceReceipt), Self::Error>;
    fn commit_verified(&self, intent: WriteIntent<CasVerified>) -> Result<String, Self::Error>;
}

/// Application composition shell parameterized over static-dispatch driven ports.
pub struct App<W: WorkspaceStore> {
    store: W,
    generation: Generation,
}

impl<W: WorkspaceStore> App<W> {
    #[must_use]
    pub const fn new(store: W) -> Self {
        Self {
            store,
            generation: Generation(0),
        }
    }

    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    #[must_use]
    pub const fn store(&self) -> &W {
        &self.store
    }
}
