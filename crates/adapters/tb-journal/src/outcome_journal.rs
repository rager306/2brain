//! Framed append-only outcome journals, one file per caller request id.
//!
//! The storage primitive behind the durable outcome registry: what a request
//! did is a sequence of framed records in a file only its writer extends. A frame
//! carries its kind, payload length and CRC, so replay either reconstructs the
//! exact sequence or refuses the file — a torn or corrupt tail never becomes a
//! shorter valid history.
//!
//! One append is one write plus `sync_all`. A failed write or sync poisons the
//! writer: the on-disk tail is then ambiguous, and extending it would risk a
//! record that never became whole. Nothing here decides what an outcome means;
//! [`RecordKind`] only records what the registry observed.
//!
// The registry slice wires this to the mutation path; until then its API is
// exercised by tests.
#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Magic marking a journal file: "2BOR", the 2brain outcome registry.
const MAGIC: [u8; 4] = *b"2BOR";
/// The only journal format this build writes and reads.
const VERSION: u16 = 1;
/// File header: magic, then the version as little-endian `u16`.
const HEADER_LEN: usize = MAGIC.len() + 2;
/// A frame's fixed part: kind `u8`, payload length `u32`, payload CRC `u32`.
const FRAME_HEADER_LEN: usize = 1 + 4 + 4;
/// Largest payload one frame may carry.
const MAX_PAYLOAD: usize = 64 * 1024;

/// A caller-generated request identity: exactly 32 lowercase hex characters.
///
/// The format is validated on the way in, which is also what makes it safe as a
/// path component: it admits no separator, no `.` and no case ambiguity.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RequestId(String);

impl RequestId {
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.len() != 32 {
            return Err(format!(
                "request id must be 32 hex characters, got {}",
                text.len()
            ));
        }
        if !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err("request id must be lowercase hex".to_owned());
        }
        Ok(Self(text.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one journal record reports the request was doing.
///
/// Terminal kinds are [`RecordKind::Committed`], [`RecordKind::Rejected`] and
/// [`RecordKind::Compensated`]; the rest are internal progress, except
/// [`RecordKind::Response`], which carries the saved terminal answer after the
/// terminal record. The wire values are stable and start at 1, so an all-zero
/// tail decodes as an unknown kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecordKind {
    Reserved = 1,
    Prepared = 2,
    Executing = 3,
    Compensating = 4,
    Committed = 5,
    Rejected = 6,
    Compensated = 7,
    /// The terminal response frame as UTF-8 JSON bytes, appended after the
    /// terminal state record. It is not a state of its own.
    Response = 8,
}

impl RecordKind {
    pub fn from_u8(value: u8) -> Result<Self, String> {
        Ok(match value {
            1 => Self::Reserved,
            2 => Self::Prepared,
            3 => Self::Executing,
            4 => Self::Compensating,
            5 => Self::Committed,
            6 => Self::Rejected,
            7 => Self::Compensated,
            8 => Self::Response,
            other => return Err(format!("unknown record kind {other}")),
        })
    }

    pub fn as_u8(self) -> u8 {
        self as u8
    }
}

/// One recorded step of a request's execution.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Record {
    pub kind: RecordKind,
    pub payload: Vec<u8>,
}

/// An open journal for one request id, positioned to append.
#[derive(Debug)]
pub struct OutcomeJournal {
    root: PathBuf,
    file: File,
    bytes_written: u64,
    marked_dirty: bool,
    /// Set once a write or sync failed: the tail is ambiguous, so no further
    /// record may be added to this file by this writer.
    poisoned: bool,
    #[cfg(test)]
    fail_next_append: bool,
}

impl OutcomeJournal {
    /// Create the journal for `id` under `root`, leaving it empty.
    ///
    /// The file is created with `create_new`, so an existing journal for the
    /// same id is [`io::ErrorKind::AlreadyExists`] rather than an overwrite.
    /// The header is synced with the file and the holding directory with it.
    pub fn create(root: &Path, id: &RequestId) -> io::Result<Self> {
        let path = journal_path(root, id);
        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "journal path has no parent")
        })?;
        fs::create_dir_all(parent)?;
        crate::outcome_registry::mark_usage_dirty(root)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&header())?;
        file.sync_all()?;
        sync_dir(parent)?;
        Ok(Self {
            root: root.to_path_buf(),
            file,
            bytes_written: HEADER_LEN as u64,
            marked_dirty: true,
            poisoned: false,
            #[cfg(test)]
            fail_next_append: false,
        })
    }

    /// Open an existing journal for appending, after validating its header.
    ///
    /// A missing file is `NotFound`; a different magic or version is
    /// `InvalidData`. Only the header is checked here — the whole file is
    /// validated by [`records`](Self::records) before a caller replays it.
    pub fn open(root: &Path, id: &RequestId) -> io::Result<Self> {
        let path = journal_path(root, id);
        let mut file = OpenOptions::new().read(true).append(true).open(&path)?;
        let mut found = [0u8; HEADER_LEN];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut found)?;
        if found != header() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "journal header names another format",
            ));
        }
        Ok(Self {
            root: root.to_path_buf(),
            file,
            bytes_written: 0,
            marked_dirty: false,
            poisoned: false,
            #[cfg(test)]
            fail_next_append: false,
        })
    }

    /// Append one record and make it durable.
    ///
    /// A write or sync that fails poisons this writer and returns the failure;
    /// every later append reports [`io::ErrorKind::Other`] instead of
    /// extending a log whose tail the caller cannot know.
    pub fn append(&mut self, kind: RecordKind, payload: &[u8]) -> io::Result<()> {
        if self.poisoned {
            return Err(poisoned_error());
        }
        if payload.len() > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("payload of {} bytes exceeds {MAX_PAYLOAD}", payload.len()),
            ));
        }
        if !self.marked_dirty {
            crate::outcome_registry::mark_usage_dirty(&self.root)?;
            self.marked_dirty = true;
        }
        #[cfg(test)]
        if std::mem::take(&mut self.fail_next_append) {
            self.poisoned = true;
            return Err(io::Error::other("injected append failure"));
        }
        let len = payload.len() as u32;
        let len_bytes = len.to_le_bytes();
        let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + payload.len());
        frame.push(kind.as_u8());
        frame.extend_from_slice(&len_bytes);
        frame.extend_from_slice(&frame_crc(kind, &len_bytes, payload).to_le_bytes());
        frame.extend_from_slice(payload);
        if let Err(error) = self
            .file
            .write_all(&frame)
            .and_then(|()| self.file.sync_all())
        {
            self.poisoned = true;
            return Err(error);
        }
        self.bytes_written = self.bytes_written.saturating_add(frame.len() as u64);
        Ok(())
    }

    /// Exact number of bytes durably written through this handle so far.
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Whether a failed write or sync left this handle's on-disk byte delta ambiguous.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Append the saved terminal response as one [`RecordKind::Response`]
    /// record. The bytes are the response frame exactly as it was returned to
    /// the caller; this layer only frames them.
    pub fn append_response(&mut self, frame: &[u8]) -> io::Result<()> {
        self.append(RecordKind::Response, frame)
    }

    /// Read the records of the journal for `id`, or `None` when no journal
    /// exists.
    ///
    /// Strict: a wrong magic or version, an unknown kind, an oversized frame, a
    /// CRC mismatch, or a truncated or zeroed tail is an error, never a shorter
    /// valid history. Reads only; nothing is created.
    pub fn records(root: &Path, id: &RequestId) -> io::Result<Option<Vec<Record>>> {
        let bytes = match fs::read(journal_path(root, id)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(parse(&bytes)?))
    }

    /// Make the next append fail as if the write did, poisoning the writer.
    #[cfg(test)]
    pub fn fail_next_append(&mut self) {
        self.fail_next_append = true;
    }
}

/// `<root>/outcomes/<first two hex>/<request id>.journal`.
fn journal_path(root: &Path, id: &RequestId) -> PathBuf {
    let id = id.as_str();
    root.join("outcomes")
        .join(&id[..2])
        .join(format!("{id}.journal"))
}

fn header() -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[..MAGIC.len()].copy_from_slice(&MAGIC);
    header[MAGIC.len()..].copy_from_slice(&VERSION.to_le_bytes());
    header
}

fn parse(bytes: &[u8]) -> io::Result<Vec<Record>> {
    if bytes.len() < HEADER_LEN {
        return Err(corrupt("journal is shorter than its header"));
    }
    if bytes[..MAGIC.len()] != MAGIC {
        return Err(corrupt("not an outcome journal"));
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != VERSION {
        return Err(corrupt(format!(
            "journal version {version} is not {VERSION}"
        )));
    }
    let mut records = Vec::new();
    let mut rest = &bytes[HEADER_LEN..];
    while !rest.is_empty() {
        if rest.len() < FRAME_HEADER_LEN {
            return Err(corrupt("truncated frame header"));
        }
        let kind = RecordKind::from_u8(rest[0]).map_err(corrupt)?;
        let len_bytes = [rest[1], rest[2], rest[3], rest[4]];
        let len = u32::from_le_bytes(len_bytes) as usize;
        let crc = u32::from_le_bytes([rest[5], rest[6], rest[7], rest[8]]);
        if len > MAX_PAYLOAD {
            return Err(corrupt(format!("frame length {len} exceeds {MAX_PAYLOAD}")));
        }
        if rest.len() < FRAME_HEADER_LEN + len {
            return Err(corrupt("truncated frame payload"));
        }
        let payload = &rest[FRAME_HEADER_LEN..FRAME_HEADER_LEN + len];
        if frame_crc(kind, &len_bytes, payload) != crc {
            return Err(corrupt("frame CRC mismatch"));
        }
        records.push(Record {
            kind,
            payload: payload.to_vec(),
        });
        rest = &rest[FRAME_HEADER_LEN + len..];
    }
    Ok(records)
}

fn corrupt(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

fn poisoned_error() -> io::Error {
    io::Error::other("journal writer is poisoned by an earlier failed append")
}

/// Directory entries become durable by syncing the directory itself, which is
/// a POSIX operation. Elsewhere the created entry is left to the platform.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

const CRC32_INIT: u32 = 0xFFFF_FFFF;
const CRC32_POLYNOMIAL: u32 = 0xEDB8_8320;
const CRC32_XOROUT: u32 = 0xFFFF_FFFF;

/// CRC-32 IEEE, reflected, one byte at a time: no table, no dependency.
fn crc32_update(state: u32, bytes: &[u8]) -> u32 {
    let mut crc = state;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (CRC32_POLYNOMIAL & mask);
        }
    }
    crc
}

pub fn crc32(bytes: &[u8]) -> u32 {
    crc32_update(CRC32_INIT, bytes) ^ CRC32_XOROUT
}

/// The CRC a frame carries: over the kind byte, the little-endian length, and
/// the payload, so both the length and the body are covered.
fn frame_crc(kind: RecordKind, len_bytes: &[u8; 4], payload: &[u8]) -> u32 {
    let state = crc32_update(CRC32_INIT, &[kind.as_u8()]);
    let state = crc32_update(state, len_bytes);
    crc32_update(state, payload) ^ CRC32_XOROUT
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0123456789abcdef0123456789abcdef";

    fn request_id() -> RequestId {
        RequestId::parse(ID).expect("valid request id")
    }

    fn journal_of(root: &Path) -> PathBuf {
        journal_path(root, &request_id())
    }

    fn frame_bytes(kind: u8, payload: &[u8]) -> Vec<u8> {
        let len_bytes = (payload.len() as u32).to_le_bytes();
        let state = crc32_update(CRC32_INIT, &[kind]);
        let state = crc32_update(state, &len_bytes);
        let crc = crc32_update(state, payload) ^ CRC32_XOROUT;
        let mut frame = Vec::new();
        frame.push(kind);
        frame.extend_from_slice(&len_bytes);
        frame.extend_from_slice(&crc.to_le_bytes());
        frame.extend_from_slice(payload);
        frame
    }

    fn write_journal(path: &Path, version: u16, frames: &[Vec<u8>]) {
        let mut bytes = MAGIC.to_vec();
        bytes.extend_from_slice(&version.to_le_bytes());
        for frame in frames {
            bytes.extend_from_slice(frame);
        }
        fs::create_dir_all(path.parent().expect("journal parent")).expect("create dirs");
        fs::write(path, bytes).expect("write journal");
    }

    #[test]
    fn roundtrip_preserves_order_and_payloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let mut journal = OutcomeJournal::create(root, &request_id()).expect("create journal");
        let steps = [
            (RecordKind::Reserved, b"".as_slice()),
            (RecordKind::Prepared, b"digest".as_slice()),
            (RecordKind::Executing, b"plan".as_slice()),
            (RecordKind::Committed, b"result".as_slice()),
        ];
        for (kind, payload) in steps {
            journal.append(kind, payload).expect("append");
        }
        let records = OutcomeJournal::records(root, &request_id())
            .expect("read journal")
            .expect("journal exists");
        let expected: Vec<Record> = steps
            .iter()
            .map(|(kind, payload)| Record {
                kind: *kind,
                payload: payload.to_vec(),
            })
            .collect();
        assert_eq!(records, expected);
    }

    #[test]
    fn created_journal_sits_under_the_first_two_hex_characters() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        OutcomeJournal::create(root, &request_id()).expect("create journal");
        assert_eq!(
            journal_of(root),
            root.join("outcomes")
                .join("01")
                .join(format!("{ID}.journal"))
        );
        assert!(journal_of(root).is_file());
        assert!(
            OutcomeJournal::records(root, &request_id())
                .expect("read journal")
                .expect("journal exists")
                .is_empty()
        );
    }

    #[test]
    fn absent_journal_is_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(
            OutcomeJournal::records(dir.path(), &request_id())
                .expect("read journal")
                .is_none()
        );
    }

    #[test]
    fn open_appends_to_an_existing_journal_and_refuses_a_foreign_header() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id = request_id();
        {
            let mut journal = OutcomeJournal::create(root, &id).unwrap();
            journal.append(RecordKind::Reserved, b"first").unwrap();
        }
        let mut reopened = OutcomeJournal::open(root, &id).unwrap();
        reopened.append(RecordKind::Executing, b"second").unwrap();
        let records = OutcomeJournal::records(root, &id).unwrap().unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].payload, b"second");

        std::fs::write(journal_path(root, &id), b"NOPE\x01\x00").unwrap();
        let error = OutcomeJournal::open(root, &id).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            OutcomeJournal::open(
                root,
                &RequestId::parse("ffffffffffffffffffffffffffffffff").unwrap()
            )
            .unwrap_err()
            .kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn duplicate_create_is_already_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let _first = OutcomeJournal::create(root, &request_id()).expect("create journal");
        let error = OutcomeJournal::create(root, &request_id()).expect_err("second create fails");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn corrupt_crc_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let mut journal = OutcomeJournal::create(root, &request_id()).expect("create journal");
        journal
            .append(RecordKind::Committed, b"done")
            .expect("append");
        drop(journal);
        let path = journal_of(root);
        let mut bytes = fs::read(&path).expect("read file");
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        fs::write(&path, &bytes).expect("write file");
        let error = OutcomeJournal::records(root, &request_id()).expect_err("corrupt CRC");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn truncated_tail_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let mut journal = OutcomeJournal::create(root, &request_id()).expect("create journal");
        journal
            .append(RecordKind::Reserved, b"one")
            .expect("append");
        journal
            .append(RecordKind::Committed, b"two")
            .expect("append");
        drop(journal);
        let path = journal_of(root);
        let bytes = fs::read(&path).expect("read file");
        fs::write(&path, &bytes[..bytes.len() - 1]).expect("truncate file");
        let error = OutcomeJournal::records(root, &request_id()).expect_err("truncated tail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn zero_tail_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let mut journal = OutcomeJournal::create(root, &request_id()).expect("create journal");
        journal
            .append(RecordKind::Executing, b"step")
            .expect("append");
        drop(journal);
        let path = journal_of(root);
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open file");
        file.write_all(&[0u8; FRAME_HEADER_LEN]).expect("zero tail");
        file.sync_all().expect("sync");
        let error = OutcomeJournal::records(root, &request_id()).expect_err("zero tail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn unknown_version_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_journal(&journal_of(root), 2, &[frame_bytes(1, b"")]);
        let error = OutcomeJournal::records(root, &request_id()).expect_err("unknown version");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("version 2"));
    }

    #[test]
    fn unknown_kind_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        write_journal(&journal_of(root), VERSION, &[frame_bytes(9, b"")]);
        let error = OutcomeJournal::records(root, &request_id()).expect_err("unknown kind");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn poisoned_writer_refuses_later_appends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let mut journal = OutcomeJournal::create(root, &request_id()).expect("create journal");
        journal
            .append(RecordKind::Reserved, b"")
            .expect("first append");
        journal.fail_next_append();
        journal
            .append(RecordKind::Executing, b"lost")
            .expect_err("injected failure");
        let error = journal
            .append(RecordKind::Committed, b"never")
            .expect_err("poisoned writer");
        assert_eq!(error.kind(), io::ErrorKind::Other);
        drop(journal);
        let records = OutcomeJournal::records(root, &request_id())
            .expect("read journal")
            .expect("journal exists");
        assert_eq!(records.len(), 1, "an ambiguous append is never persisted");
        assert_eq!(records[0].kind, RecordKind::Reserved);
    }

    #[test]
    fn oversized_payload_is_refused_without_poisoning() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut journal =
            OutcomeJournal::create(dir.path(), &request_id()).expect("create journal");
        let oversized = vec![0u8; MAX_PAYLOAD + 1];
        let error = journal
            .append(RecordKind::Prepared, &oversized)
            .expect_err("payload too large");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        journal
            .append(RecordKind::Committed, b"ok")
            .expect("writer still usable");
    }

    #[test]
    fn response_kind_roundtrips_and_is_not_a_state() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let mut journal = OutcomeJournal::create(root, &request_id()).expect("create journal");
        journal.append(RecordKind::Committed, b"").expect("append");
        journal
            .append_response(b"{\"a\":1}")
            .expect("append response");
        let records = OutcomeJournal::records(root, &request_id())
            .expect("read journal")
            .expect("journal exists");
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].kind, RecordKind::Response);
        assert_eq!(records[1].payload, b"{\"a\":1}");
        assert_eq!(RecordKind::from_u8(8), Ok(RecordKind::Response));
        assert!(RecordKind::from_u8(9).is_err(), "kind 9 is still unknown");
    }

    #[test]
    fn crc32_matches_ieee_vector() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn request_id_rejects_nonconforming_text() {
        assert!(RequestId::parse("0123456789abcdef0123456789abcde").is_err());
        assert!(RequestId::parse("0123456789ABCDEF0123456789ABCDEF").is_err());
        assert!(RequestId::parse("0123456789abcdef0123456789abcdeg").is_err());
        assert!(RequestId::parse("../0123456789abcdef0123456789ab").is_err());
        assert_eq!(request_id().as_str(), ID);
    }
}
