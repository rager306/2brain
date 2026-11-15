//! The read-before-write ledger.
//!
//! A mutation may only proceed on a file this process has actually served, and only when
//! the hash it was based on is still the current one. The ledger is that record: keyed by
//! (member, member-relative path), holding the hash the read answered with.
//!
//! Two refusals come out of it and they are deliberately different codes:
//! `policy/prior_read` when nothing was ever served, `policy/freshness` when something was
//! served but the file has moved on since.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// (member, member-relative path) — the key a read and a mutation agree on.
pub(crate) type Key = (String, String);

/// What this process has served, and with which hash.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReadLedger {
    served: Arc<Mutex<HashMap<Key, String>>>,
}

impl ReadLedger {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Remember that `key` was served with `hash`.
    pub(crate) fn record(&self, key: Key, hash: String) {
        self.served
            .lock()
            .expect("ledger lock poisoned")
            .insert(key, hash);
    }

    /// The hash this process last served for `key`, or None when it never did.
    pub(crate) fn recorded(&self, key: &Key) -> Option<String> {
        self.served
            .lock()
            .expect("ledger lock poisoned")
            .get(key)
            .cloned()
    }

    /// Drop `key`: the file either changed under us or a mutation consumed the read.
    pub(crate) fn forget(&self, key: &Key) {
        self.served
            .lock()
            .expect("ledger lock poisoned")
            .remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str) -> Key {
        ("vault".to_owned(), path.to_owned())
    }

    #[test]
    fn a_recorded_read_is_returned_and_can_be_forgotten() {
        let ledger = ReadLedger::new();
        assert_eq!(ledger.recorded(&key("inbox/a.md")), None);
        ledger.record(key("inbox/a.md"), "abc123".to_owned());
        assert_eq!(
            ledger.recorded(&key("inbox/a.md")).as_deref(),
            Some("abc123")
        );
        ledger.forget(&key("inbox/a.md"));
        assert_eq!(
            ledger.recorded(&key("inbox/a.md")),
            None,
            "a consumed read must not arm the next mutation"
        );
    }

    #[test]
    fn members_and_paths_are_separate_entries() {
        let ledger = ReadLedger::new();
        ledger.record(("vault".to_owned(), "a.md".to_owned()), "one".to_owned());
        ledger.record(("engine".to_owned(), "a.md".to_owned()), "two".to_owned());
        assert_eq!(
            ledger
                .recorded(&("vault".to_owned(), "a.md".to_owned()))
                .as_deref(),
            Some("one")
        );
        assert_eq!(
            ledger
                .recorded(&("engine".to_owned(), "a.md".to_owned()))
                .as_deref(),
            Some("two")
        );
    }

    #[test]
    fn a_clone_shares_the_same_ledger() {
        let ledger = ReadLedger::new();
        let clone = ledger.clone();
        clone.record(key("b.md"), "h".to_owned());
        assert_eq!(
            ledger.recorded(&key("b.md")).as_deref(),
            Some("h"),
            "the server is cloned per request, so the ledger must be shared"
        );
    }
}
