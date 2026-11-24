//! ADR 0309 — Consensus-backed replication log.
//!
//! Ships the single-replica-leader case of a Raft-style
//! consensus log: an append-only, durably persisted log
//! of byte payloads addressed by monotone `LogIndex`.
//! `append` writes a length-prefixed + CRC-stamped record
//! to the file and `sync_data`s before returning the
//! committed index, so an acknowledged append survives
//! crash and restart.
//!
//! Multi-replica replication (vote RPCs, replication of
//! entries to peers, leader election, membership change)
//! remains a follow-up ADR; the surface here is already
//! what that layer will plug into — every peer's append
//! is a monotonic sequence of these records and every
//! replica's log is one of these files.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct ReplicaId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct Term(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct LogIndex(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role { Follower, Candidate, Leader }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub term:  Term,
    pub index: LogIndex,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("log record at offset {offset} is corrupt: {kind}")]
    Corrupt { offset: u64, kind: &'static str },
}
pub type ConsensusResult<T> = Result<T, ConsensusError>;

pub trait ConsensusGroup {
    fn append(&self, bytes: Vec<u8>) -> ConsensusResult<LogIndex>;
    fn committed_index(&self) -> LogIndex;
    fn role(&self) -> Role;
}

/// In-memory log, no disk. Good for tests + any
/// coordinator's own ephemeral in-process state.
#[derive(Debug, Default)]
pub struct InMemoryConsensusGroup {
    log: Mutex<Vec<LogEntry>>,
}

impl InMemoryConsensusGroup {
    pub fn new() -> Self { Self::default() }
}

impl ConsensusGroup for InMemoryConsensusGroup {
    fn append(&self, bytes: Vec<u8>) -> ConsensusResult<LogIndex> {
        let mut log = self.log.lock();
        let index = LogIndex(log.len() as u64 + 1);
        log.push(LogEntry { term: Term(1), index, bytes });
        Ok(index)
    }
    fn committed_index(&self) -> LogIndex {
        LogIndex(self.log.lock().len() as u64)
    }
    fn role(&self) -> Role { Role::Leader }
}

// ---------------------------------------------------------------------------
// LocalConsensusLog — durable append-only on-disk log.

const FILE_MAGIC:    &[u8; 8] = b"BRCL\x00\x00\x00\x01";
const RECORD_MAGIC:  &[u8; 4] = b"CREC";
const RECORD_HEADER_LEN: usize = 24;   // magic(4) + term(8) + index(8) + len(4)
const RECORD_CRC_LEN:    usize = 4;

/// Durable consensus log. Entries are durably persisted
/// before `append` returns. On `open`, trailing torn
/// records (incomplete header, truncated payload, CRC
/// mismatch) are **truncated** so the log always presents
/// a consistent prefix.
#[derive(Debug)]
pub struct LocalConsensusLog {
    path:  PathBuf,
    inner: Mutex<LocalInner>,
}

#[derive(Debug)]
struct LocalInner {
    file:       File,
    log:        Vec<LogEntry>,
    term:       Term,
    cached_len: u64,
}

impl LocalConsensusLog {
    /// Open or create a durable log at `path`. If the file
    /// exists, replay it into memory and truncate any torn
    /// trailing record.
    pub fn open(path: impl Into<PathBuf>) -> ConsensusResult<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }

        let exists = path.is_file();
        let mut file = OpenOptions::new()
            .read(true).write(true).create(true).truncate(false)
            .open(&path)?;
        let (log, cached_len) = if !exists {
            file.write_all(FILE_MAGIC)?;
            file.sync_data()?;
            (Vec::new(), FILE_MAGIC.len() as u64)
        } else {
            let (entries, consistent_end) = replay(&mut file)?;
            let file_len = file.metadata()?.len();
            if consistent_end < file_len {
                file.set_len(consistent_end)?;
                file.sync_data()?;
            }
            (entries, consistent_end)
        };
        Ok(Self {
            path,
            inner: Mutex::new(LocalInner {
                file, log, term: Term(1), cached_len,
            }),
        })
    }

    pub fn path(&self) -> &Path { &self.path }
    pub fn file_len(&self) -> u64 { self.inner.lock().cached_len }
    pub fn len_entries(&self) -> usize { self.inner.lock().log.len() }

    pub fn entry(&self, index: LogIndex) -> Option<LogEntry> {
        let g = self.inner.lock();
        let pos = index.0 as usize;
        if pos == 0 || pos > g.log.len() { None } else { Some(g.log[pos - 1].clone()) }
    }
}

impl ConsensusGroup for LocalConsensusLog {
    fn append(&self, bytes: Vec<u8>) -> ConsensusResult<LogIndex> {
        let mut g = self.inner.lock();
        let index = LogIndex(g.log.len() as u64 + 1);
        let term  = g.term;
        let record = encode_record(term, index, &bytes);
        g.file.seek(SeekFrom::End(0))?;
        g.file.write_all(&record)?;
        g.file.sync_data()?;
        g.cached_len += record.len() as u64;
        g.log.push(LogEntry { term, index, bytes });
        Ok(index)
    }

    fn committed_index(&self) -> LogIndex {
        LogIndex(self.inner.lock().log.len() as u64)
    }

    fn role(&self) -> Role { Role::Leader }
}

fn encode_record(term: Term, index: LogIndex, payload: &[u8]) -> Vec<u8> {
    let payload_len: u32 = payload.len() as u32;
    let mut out = Vec::with_capacity(RECORD_HEADER_LEN + payload.len() + RECORD_CRC_LEN);
    out.extend_from_slice(RECORD_MAGIC);
    out.extend_from_slice(&term.0.to_le_bytes());
    out.extend_from_slice(&index.0.to_le_bytes());
    out.extend_from_slice(&payload_len.to_le_bytes());
    out.extend_from_slice(payload);
    let crc = crc32fast::hash(&out[4..]);   // skip record magic
    out.extend_from_slice(&crc.to_le_bytes());
    out
}

fn replay(file: &mut File) -> ConsensusResult<(Vec<LogEntry>, u64)> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = BufReader::new(file.try_clone()?);
    let mut magic  = [0u8; 8];
    reader.read_exact(&mut magic)
        .map_err(|_| ConsensusError::Corrupt { offset: 0, kind: "file magic" })?;
    if magic != *FILE_MAGIC {
        return Err(ConsensusError::Corrupt { offset: 0, kind: "file magic" });
    }
    let mut consistent_end = FILE_MAGIC.len() as u64;
    let mut entries        = Vec::new();

    loop {
        let before_record = consistent_end;
        let mut hdr = [0u8; RECORD_HEADER_LEN];
        match reader.read_exact(&mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        if &hdr[0..4] != RECORD_MAGIC { break; }
        let term_u64    = u64::from_le_bytes(hdr[4..12].try_into().unwrap());
        let index_u64   = u64::from_le_bytes(hdr[12..20].try_into().unwrap());
        let payload_len = u32::from_le_bytes(hdr[20..24].try_into().unwrap()) as usize;

        let mut payload = vec![0u8; payload_len];
        if reader.read_exact(&mut payload).is_err() { break; }

        let mut crc_bytes = [0u8; RECORD_CRC_LEN];
        if reader.read_exact(&mut crc_bytes).is_err() { break; }
        let expected_crc = u32::from_le_bytes(crc_bytes);

        let mut acc = Vec::with_capacity(20 + payload.len());
        acc.extend_from_slice(&term_u64.to_le_bytes());
        acc.extend_from_slice(&index_u64.to_le_bytes());
        acc.extend_from_slice(&(payload_len as u32).to_le_bytes());
        acc.extend_from_slice(&payload);
        let actual_crc = crc32fast::hash(&acc);
        if actual_crc != expected_crc { break; }

        let expected_index = entries.len() as u64 + 1;
        if index_u64 != expected_index {
            return Err(ConsensusError::Corrupt {
                offset: before_record, kind: "non-monotonic index" });
        }
        entries.push(LogEntry {
            term:  Term(term_u64),
            index: LogIndex(index_u64),
            bytes: payload,
        });
        consistent_end += RECORD_HEADER_LEN as u64
            + payload_len as u64
            + RECORD_CRC_LEN as u64;
    }
    Ok((entries, consistent_end))
}

// ---------------------------------------------------------------------------

/// Group registry so a coordinator can keep per-shard
/// consensus logs under one root.
#[derive(Debug, Default)]
pub struct LocalConsensusRegistry {
    root:   PathBuf,
    groups: Mutex<HashMap<String, std::sync::Arc<LocalConsensusLog>>>,
}

impl LocalConsensusRegistry {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), groups: Mutex::new(HashMap::new()) }
    }

    pub fn open_group(&self, name: &str)
        -> ConsensusResult<std::sync::Arc<LocalConsensusLog>>
    {
        let mut g = self.groups.lock();
        if let Some(l) = g.get(name) { return Ok(l.clone()); }
        let path = self.root.join(format!("{name}.consensus.log"));
        let log  = std::sync::Arc::new(LocalConsensusLog::open(path)?);
        g.insert(name.to_string(), log.clone());
        Ok(log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_advances_committed_index_monotonically() {
        let g = InMemoryConsensusGroup::new();
        assert_eq!(g.committed_index(), LogIndex(0));
        let i1 = g.append(vec![1, 2, 3]).unwrap();
        let i2 = g.append(vec![4, 5, 6]).unwrap();
        assert_eq!(i1, LogIndex(1));
        assert_eq!(i2, LogIndex(2));
        assert_eq!(g.committed_index(), LogIndex(2));
    }

    #[test]
    fn single_replica_stub_is_always_leader() {
        let g = InMemoryConsensusGroup::new();
        assert_eq!(g.role(), Role::Leader);
        g.append(vec![0]).unwrap();
        assert_eq!(g.role(), Role::Leader);
    }

    #[test]
    fn local_log_persists_appends_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("group.log");
        {
            let log = LocalConsensusLog::open(&path).unwrap();
            for i in 0u64..32 {
                let idx = log.append(format!("rec-{i}").into_bytes()).unwrap();
                assert_eq!(idx, LogIndex(i + 1));
            }
            assert_eq!(log.committed_index(), LogIndex(32));
        }
        let reopened = LocalConsensusLog::open(&path).unwrap();
        assert_eq!(reopened.committed_index(), LogIndex(32));
        for i in 0u64..32 {
            let entry = reopened.entry(LogIndex(i + 1)).unwrap();
            assert_eq!(entry.index, LogIndex(i + 1));
            assert_eq!(entry.bytes, format!("rec-{i}").into_bytes());
        }
    }

    #[test]
    fn append_after_reopen_continues_the_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("seq.log");
        {
            let log = LocalConsensusLog::open(&path).unwrap();
            log.append(b"a".to_vec()).unwrap();
            log.append(b"b".to_vec()).unwrap();
        }
        let log = LocalConsensusLog::open(&path).unwrap();
        let third = log.append(b"c".to_vec()).unwrap();
        assert_eq!(third, LogIndex(3));
    }

    #[test]
    fn torn_trailing_record_is_truncated_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("torn.log");
        {
            let log = LocalConsensusLog::open(&path).unwrap();
            log.append(b"first".to_vec()).unwrap();
            log.append(b"second".to_vec()).unwrap();
        }
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"TORN-TAIL").unwrap();
            f.sync_data().unwrap();
        }
        let len_before = std::fs::metadata(&path).unwrap().len();
        let log = LocalConsensusLog::open(&path).unwrap();
        assert_eq!(log.committed_index(), LogIndex(2));
        let len_after = std::fs::metadata(&path).unwrap().len();
        assert!(len_after < len_before,
            "torn tail should be trimmed: before={len_before} after={len_after}");
    }

    #[test]
    fn crc_mismatch_on_last_record_is_truncated_as_torn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crc.log");
        {
            let log = LocalConsensusLog::open(&path).unwrap();
            log.append(b"ok".to_vec()).unwrap();
            log.append(b"doomed".to_vec()).unwrap();
        }
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 5;
        bytes[last] ^= 0xff;
        std::fs::write(&path, &bytes).unwrap();
        let log = LocalConsensusLog::open(&path).unwrap();
        assert_eq!(log.committed_index(), LogIndex(1));
        assert_eq!(log.entry(LogIndex(1)).unwrap().bytes, b"ok");
    }

    #[test]
    fn missing_file_magic_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("badmagic.log");
        std::fs::write(&path, b"notBRCL1").unwrap();  // 8 bytes but wrong
        let err = LocalConsensusLog::open(&path).unwrap_err();
        assert!(matches!(err, ConsensusError::Corrupt { kind: "file magic", .. }),
            "got {err:?}");
    }

    #[test]
    fn registry_opens_same_group_once_and_caches() {
        let dir = tempfile::tempdir().unwrap();
        let reg = LocalConsensusRegistry::new(dir.path());
        let a = reg.open_group("shard-7").unwrap();
        a.append(b"x".to_vec()).unwrap();
        let b = reg.open_group("shard-7").unwrap();
        assert_eq!(b.committed_index(), LogIndex(1),
            "second open_group must return the same live handle");
        let other = reg.open_group("shard-8").unwrap();
        assert_eq!(other.committed_index(), LogIndex(0));
    }
}
