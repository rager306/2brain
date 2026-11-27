//! Pure domain contracts for 2brain v2 (0 I/O, Miri-clean).
//!
//! Encodes the five separate identity axes (ObjectId/NodeId, Locator,
//! RevisionDigest, Generation, RequestId), typed uncertainty (`Assessment<T>`),
//! compile-time typestate write gate (`WriteIntent<S>`), and the durable
//! outcome state machine (`OutcomeState`).

use std::fmt;
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Compact interned graph node identifier (zero-overhead `u32`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct NodeId(pub u32);

/// Compact interned vocabulary term identifier (zero-overhead `u32`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TermId(pub u32);

/// Compact interned schema type identifier (zero-overhead `u16`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TypeId(pub u16);

/// Monotonically increasing derived snapshot generation (`KTD5`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[repr(transparent)]
pub struct Generation(pub u64);

impl Generation {
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

/// Canonical `(repo, relative_posix_path)` locator — distinct from identity and bytes.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Locator {
    pub repo: String,
    pub path: String,
}

impl Locator {
    pub fn new(repo: impl Into<String>, path: impl Into<String>) -> Result<Self, ContractError> {
        let repo = repo.into();
        let path = path.into();
        if repo.is_empty() || path.is_empty() {
            return Err(ContractError::InvalidLocator(
                "repo and path must be non-empty".into(),
            ));
        }
        if path.starts_with('/') || path.split('/').any(|seg| seg == ".." || seg == ".") {
            return Err(ContractError::InvalidLocator(format!(
                "path must be clean relative POSIX: {path}"
            )));
        }
        Ok(Self { repo, path })
    }
}

/// Exact SHA-256 digest of a file's full bytes (`D02`).
#[derive(Copy, Clone, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct RevisionDigest([u8; 32]);

impl RevisionDigest {
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        let hash = Sha256::digest(bytes);
        let mut out = [0u8; 32];
        out.copy_from_slice(&hash);
        Self(out)
    }

    pub fn from_hex(hex: &str) -> Result<Self, ContractError> {
        if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(ContractError::InvalidDigest(hex.to_owned()));
        }
        let mut out = [0u8; 32];
        for (idx, chunk) in hex.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            out[idx] = (hex_nibble(chunk[0]) << 4) | hex_nibble(chunk[1]);
        }
        Ok(Self(out))
    }

    #[must_use]
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(64);
        for &byte in &self.0 {
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
        out
    }
}

impl fmt::Debug for RevisionDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RevisionDigest({})", self.to_hex())
    }
}

/// Whole-file FNV-1a 64-bit write-guard digest (`expected_hash`).
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct WriteGuardHash(pub u64);

impl WriteGuardHash {
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let mut h = OFFSET_BASIS;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(PRIME);
        }
        Self(h)
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        format!("{:016x}", self.0)
    }
}

/// Caller-supplied 32-lowercase-hex idempotency request identifier (`ADR-0009`).
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RequestId(String);

impl RequestId {
    pub fn parse(raw: &str) -> Result<Self, ContractError> {
        if raw.len() != 32 || !raw.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(ContractError::InvalidRequestId(raw.to_owned()));
        }
        Ok(Self(raw.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Typed uncertainty for every explanation or retrieval dimension (`KTD9`, `R8`).
///
/// Prevents coercing an unavailable check into an empty list (`[]`).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Assessment<T> {
    Available { items: T },
    AssessedEmpty,
    Incomplete { partial: T, reason: String },
    Unavailable { reason: String },
    Unassessed,
}

/// Exact same-buffer observation receipt (`D02`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvidenceReceipt {
    pub locator: Locator,
    pub sha256: RevisionDigest,
    pub write_guard: WriteGuardHash,
    pub total_bytes: usize,
    pub byte_start: usize,
    pub byte_end: usize,
    pub truncated: bool,
    pub generation: Generation,
}

impl EvidenceReceipt {
    #[must_use]
    pub fn is_complete_file(&self) -> bool {
        !self.truncated && self.byte_start == 0 && self.byte_end == self.total_bytes
    }
}

// === Compile-time Typestate Write Gate (SEM-02 + CAS) ===

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Unchecked;
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ReadVerified;
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CasVerified;

/// Typestate write intent: impossible at compile time to commit without passing
/// both a complete-file read verification and a compare-and-set check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteIntent<State> {
    locator: Locator,
    expected_guard: WriteGuardHash,
    new_content: String,
    _state: PhantomData<State>,
}

impl WriteIntent<Unchecked> {
    #[must_use]
    pub fn new(locator: Locator, expected_guard: WriteGuardHash, new_content: String) -> Self {
        Self {
            locator,
            expected_guard,
            new_content,
            _state: PhantomData,
        }
    }

    pub fn verify_full_read(
        self,
        receipt: &EvidenceReceipt,
    ) -> Result<WriteIntent<ReadVerified>, ContractError> {
        if receipt.locator != self.locator {
            return Err(ContractError::PriorReadMismatch);
        }
        if !receipt.is_complete_file() {
            return Err(ContractError::PartialReadCannotArmWrite);
        }
        if receipt.write_guard != self.expected_guard {
            return Err(ContractError::ExpectedHashMismatch {
                expected: self.expected_guard.to_hex(),
                actual: receipt.write_guard.to_hex(),
            });
        }
        Ok(WriteIntent {
            locator: self.locator,
            expected_guard: self.expected_guard,
            new_content: self.new_content,
            _state: PhantomData,
        })
    }
}

impl WriteIntent<ReadVerified> {
    pub fn verify_disk_cas(
        self,
        current_disk_guard: WriteGuardHash,
    ) -> Result<WriteIntent<CasVerified>, ContractError> {
        if current_disk_guard != self.expected_guard {
            return Err(ContractError::ExpectedHashMismatch {
                expected: self.expected_guard.to_hex(),
                actual: current_disk_guard.to_hex(),
            });
        }
        Ok(WriteIntent {
            locator: self.locator,
            expected_guard: self.expected_guard,
            new_content: self.new_content,
            _state: PhantomData,
        })
    }
}

impl WriteIntent<CasVerified> {
    #[must_use]
    pub fn locator(&self) -> &Locator {
        &self.locator
    }

    #[must_use]
    pub fn expected_guard(&self) -> WriteGuardHash {
        self.expected_guard
    }

    #[must_use]
    pub fn new_content(&self) -> &str {
        &self.new_content
    }
}

// === Durable Outcome Registry State Machine (ADR-0009 / D4) ===

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeState {
    Unknown,
    Reserved,
    Prepared,
    Executing,
    Compensating,
    Committed,
    Rejected,
    Compensated,
}

impl OutcomeState {
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::Rejected | Self::Compensated)
    }

    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        use OutcomeState::{
            Committed, Compensated, Compensating, Executing, Prepared, Rejected, Reserved,
        };
        matches!(
            (self, next),
            (Reserved, Prepared | Rejected)
                | (Prepared, Executing | Rejected | Compensating | Committed)
                | (Executing, Compensating | Committed)
                | (Compensating, Compensated)
        )
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ContractError {
    #[error("invalid locator: {0}")]
    InvalidLocator(String),
    #[error("invalid sha256 digest: {0}")]
    InvalidDigest(String),
    #[error("invalid request_id (expected 32 lowercase hex): {0}")]
    InvalidRequestId(String),
    #[error("prior read receipt targets a different file")]
    PriorReadMismatch,
    #[error("partial or truncated read cannot authorize a write")]
    PartialReadCannotArmWrite,
    #[error("expected_hash mismatch: expected {expected}, actual {actual}")]
    ExpectedHashMismatch { expected: String, actual: String },
}

const fn hex_nibble(byte: u8) -> u8 {
    match byte {
        b'0'..=b'9' => byte - b'0',
        b'a'..=b'f' => byte - b'a' + 10,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locator_rejects_traversal_and_absolute_paths() {
        assert!(Locator::new("vault", "ops/note.md").is_ok());
        assert!(Locator::new("", "ops/note.md").is_err());
        assert!(Locator::new("vault", "/etc/passwd").is_err());
        assert!(Locator::new("vault", "../secret.md").is_err());
        assert!(Locator::new("vault", "ops/./note.md").is_err());
    }

    #[test]
    fn digest_roundtrips_and_rejects_malformed_hex() {
        let d = RevisionDigest::of_bytes(b"hello 2brain");
        let hex = d.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(RevisionDigest::from_hex(&hex).unwrap(), d);
        assert!(RevisionDigest::from_hex("ABCD").is_err());
        assert!(RevisionDigest::from_hex(&hex.to_uppercase()).is_err());
    }

    #[test]
    fn typestate_write_gate_blocks_partial_reads_and_stale_cas() {
        let loc = Locator::new("vault", "ops/task.md").unwrap();
        let bytes = b"original note bytes";
        let guard = WriteGuardHash::of_bytes(bytes);

        let partial = EvidenceReceipt {
            locator: loc.clone(),
            sha256: RevisionDigest::of_bytes(bytes),
            write_guard: guard,
            total_bytes: bytes.len(),
            byte_start: 0,
            byte_end: 8,
            truncated: true,
            generation: Generation(1),
        };
        assert_eq!(
            WriteIntent::new(loc.clone(), guard, "next".into()).verify_full_read(&partial),
            Err(ContractError::PartialReadCannotArmWrite)
        );

        let full = EvidenceReceipt {
            byte_end: bytes.len(),
            truncated: false,
            ..partial
        };
        let armed = WriteIntent::new(loc.clone(), guard, "next".into())
            .verify_full_read(&full)
            .unwrap();
        let stale_disk = WriteGuardHash::of_bytes(b"changed concurrently");
        assert!(matches!(
            armed.clone().verify_disk_cas(stale_disk),
            Err(ContractError::ExpectedHashMismatch { .. })
        ));
        let verified = armed.verify_disk_cas(guard).unwrap();
        assert_eq!(verified.locator(), &loc);
        assert_eq!(verified.new_content(), "next");
    }

    #[test]
    fn outcome_state_transitions_enforce_terminal_monotonicity() {
        assert!(OutcomeState::Reserved.can_transition_to(OutcomeState::Prepared));
        assert!(OutcomeState::Prepared.can_transition_to(OutcomeState::Committed));
        assert!(OutcomeState::Compensating.can_transition_to(OutcomeState::Compensated));
        assert!(!OutcomeState::Committed.can_transition_to(OutcomeState::Rejected));
        assert!(!OutcomeState::Rejected.can_transition_to(OutcomeState::Committed));
        assert!(!OutcomeState::Unknown.can_transition_to(OutcomeState::Reserved));
    }
}
