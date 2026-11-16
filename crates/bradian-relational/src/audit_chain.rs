//! ADR 0118 — hash-chained audit log for tamper evidence.
//!
//! Each record's `prev_hash` is the BLAKE3 hash of the
//! previous record's full bytes. A single-record tamper
//! breaks the chain detectably at the next read.
//!
//! Honest scope: tamper-EVIDENCE only, not tamper-PREVENTION.
//! An attacker who can write the file can rebuild the chain
//! end-to-end if they also have write access to the full
//! historical log. BRD-SEC-005 production obligations
//! (external anchoring via signed digests, WORM storage,
//! independent retention) remain OPEN.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAGIC: &[u8; 4] = b"BACH";    // Bradian Audit CHain
const FORMAT_VERSION: u8 = 1;
const HASH_SIZE: usize = 32;
const GENESIS_HASH: [u8; HASH_SIZE] = [0u8; HASH_SIZE];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainedAuditEntry {
    pub timestamp_ms: i64,
    pub principal_id: String,
    pub sql: String,
    pub outcome: ChainedOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChainedOutcome {
    Ok,
    Err(String),
    AuthzDenied(String),
}

/// A record on disk: prev_hash (32) + body_len (4) + body +
/// this_hash (32). The chain is BLAKE3(prev_hash || body).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChainedRecord {
    pub prev_hash: [u8; HASH_SIZE],
    pub body: ChainedAuditEntry,
    pub this_hash: [u8; HASH_SIZE],
}

#[derive(Debug, Error)]
pub enum ChainError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("corruption: {0}")]
    Corruption(String),
    #[error("tamper detected: record {index} declared prev {declared:?} but actual {actual:?}")]
    Tamper { index: usize, declared: [u8; HASH_SIZE], actual: [u8; HASH_SIZE] },
    #[error("serialization: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("record exceeds max size")]
    RecordTooLarge,
}

pub type Result<T> = std::result::Result<T, ChainError>;

const MAX_BODY_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone)]
pub struct AuditChain {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    file: File,
    path: PathBuf,
    last_hash: [u8; HASH_SIZE],
}

impl AuditChain {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let mut file = OpenOptions::new().read(true).create(true).append(true)
            .open(&path)?;
        // Walk the file to find last_hash. Fresh file → GENESIS_HASH.
        file.seek(SeekFrom::Start(0))?;
        let (last_hash, _records) = walk(&mut file)?;
        let inner = Inner { file, path, last_hash };
        Ok(Self { inner: Arc::new(Mutex::new(inner)) })
    }

    pub fn append(&self, entry: &ChainedAuditEntry) -> Result<[u8; HASH_SIZE]> {
        let body = serde_json::to_vec(entry)?;
        if body.len() > MAX_BODY_BYTES {
            return Err(ChainError::RecordTooLarge);
        }
        let mut inner = self.inner.lock();
        let prev = inner.last_hash;
        let mut hasher = blake3::Hasher::new();
        hasher.update(&prev);
        hasher.update(&body);
        let mut this_hash = [0u8; HASH_SIZE];
        this_hash.copy_from_slice(&hasher.finalize().as_bytes()[..HASH_SIZE]);

        let body_len = u32::try_from(body.len())
            .map_err(|_| ChainError::RecordTooLarge)?;
        let mut rec = Vec::with_capacity(
            4 + 1 + HASH_SIZE + 4 + body.len() + HASH_SIZE
        );
        rec.extend_from_slice(MAGIC);
        rec.push(FORMAT_VERSION);
        rec.extend_from_slice(&prev);
        rec.extend_from_slice(&body_len.to_le_bytes());
        rec.extend_from_slice(&body);
        rec.extend_from_slice(&this_hash);

        inner.file.write_all(&rec)?;
        inner.file.sync_all()?;
        inner.last_hash = this_hash;
        Ok(this_hash)
    }

    /// Walk the chain and verify every link. Returns the
    /// records on success. On tamper, returns `Tamper` with
    /// the index of the first broken link.
    pub fn verify(&self) -> Result<Vec<ChainedRecord>> {
        let path = self.inner.lock().path.clone();
        let mut file = File::open(&path)?;
        let (_last, records) = walk_with_verify(&mut file)?;
        Ok(records)
    }

    pub fn last_hash(&self) -> [u8; HASH_SIZE] { self.inner.lock().last_hash }
    pub fn path(&self) -> PathBuf { self.inner.lock().path.clone() }
}

/// Walk the file without verifying (used on open to find
/// last_hash). Still rejects obvious corruption.
fn walk(file: &mut File) -> Result<([u8; HASH_SIZE], usize)> {
    file.seek(SeekFrom::Start(0))?;
    let mut last = GENESIS_HASH;
    let mut count = 0;
    loop {
        let mut head = [0u8; 4 + 1 + HASH_SIZE + 4];
        match file.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(ChainError::Io(e)),
        }
        if &head[0..4] != MAGIC {
            return Err(ChainError::Corruption("bad record magic".into()));
        }
        if head[4] != FORMAT_VERSION {
            return Err(ChainError::Corruption(format!(
                "unsupported record version {}", head[4],
            )));
        }
        let body_len = u32::from_le_bytes(head[37..41].try_into().unwrap()) as usize;
        let mut body = vec![0u8; body_len];
        if file.read_exact(&mut body).is_err() { break; }
        let mut this = [0u8; HASH_SIZE];
        if file.read_exact(&mut this).is_err() { break; }
        last = this;
        count += 1;
    }
    Ok((last, count))
}

fn walk_with_verify(file: &mut File) -> Result<([u8; HASH_SIZE], Vec<ChainedRecord>)> {
    file.seek(SeekFrom::Start(0))?;
    let mut prev_expected = GENESIS_HASH;
    let mut records = Vec::new();
    loop {
        let mut head = [0u8; 4 + 1 + HASH_SIZE + 4];
        match file.read_exact(&mut head) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(ChainError::Io(e)),
        }
        if &head[0..4] != MAGIC {
            return Err(ChainError::Corruption("bad record magic".into()));
        }
        let mut declared_prev = [0u8; HASH_SIZE];
        declared_prev.copy_from_slice(&head[5..37]);
        if declared_prev != prev_expected {
            return Err(ChainError::Tamper {
                index: records.len(),
                declared: declared_prev,
                actual: prev_expected,
            });
        }
        let body_len = u32::from_le_bytes(head[37..41].try_into().unwrap()) as usize;
        let mut body_bytes = vec![0u8; body_len];
        file.read_exact(&mut body_bytes)?;
        let body: ChainedAuditEntry = serde_json::from_slice(&body_bytes)?;
        let mut this = [0u8; HASH_SIZE];
        file.read_exact(&mut this)?;
        // Recompute and compare.
        let mut hasher = blake3::Hasher::new();
        hasher.update(&declared_prev);
        hasher.update(&body_bytes);
        let mut computed = [0u8; HASH_SIZE];
        computed.copy_from_slice(&hasher.finalize().as_bytes()[..HASH_SIZE]);
        if computed != this {
            return Err(ChainError::Tamper {
                index: records.len(),
                declared: this,
                actual: computed,
            });
        }
        records.push(ChainedRecord { prev_hash: declared_prev, body, this_hash: this });
        prev_expected = this;
    }
    Ok((prev_expected, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(who: &str, sql: &str) -> ChainedAuditEntry {
        ChainedAuditEntry {
            timestamp_ms: 0,
            principal_id: who.into(),
            sql: sql.into(),
            outcome: ChainedOutcome::Ok,
        }
    }

    #[test]
    fn fresh_chain_starts_at_genesis() {
        let dir = tempdir().unwrap();
        let chain = AuditChain::open(dir.path().join("audit.chain")).unwrap();
        assert_eq!(chain.last_hash(), GENESIS_HASH);
    }

    #[test]
    fn append_and_verify_round_trip() {
        let dir = tempdir().unwrap();
        let chain = AuditChain::open(dir.path().join("audit.chain")).unwrap();
        chain.append(&entry("root", "SELECT 1")).unwrap();
        chain.append(&entry("root", "SELECT 2")).unwrap();
        let recs = chain.verify().unwrap();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].prev_hash, GENESIS_HASH);
        assert_eq!(recs[1].prev_hash, recs[0].this_hash);
    }

    #[test]
    fn tamper_body_breaks_chain() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.chain");
        let chain = AuditChain::open(&path).unwrap();
        chain.append(&entry("root", "SELECT 1")).unwrap();
        chain.append(&entry("root", "SELECT 2")).unwrap();
        // Flip a byte inside the first record's body. May
        // land either inside JSON structure (→ Serde parse
        // fail) or inside a string value (→ hash mismatch).
        // Both are tamper-detect outcomes — either is a pass.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4 + 1 + 32 + 4 + 2] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let err = chain.verify().unwrap_err();
        assert!(
            matches!(err, ChainError::Tamper { .. }
                        | ChainError::Serde(_)
                        | ChainError::Corruption(_)),
            "expected tamper-detect error; got {err:?}",
        );
    }

    #[test]
    fn tamper_hash_field_breaks_chain() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.chain");
        let chain = AuditChain::open(&path).unwrap();
        chain.append(&entry("root", "SELECT 1")).unwrap();
        chain.append(&entry("root", "SELECT 2")).unwrap();
        // Flip a byte in the first record's this_hash trailer.
        // record 0 ends at magic(4)+ver(1)+prev(32)+len(4)+body_len+32
        let bytes = std::fs::read(&path).unwrap();
        let body0_len_offset = 4 + 1 + 32;
        let body0_len = u32::from_le_bytes(
            bytes[body0_len_offset..body0_len_offset + 4].try_into().unwrap()
        ) as usize;
        let this_hash_offset = 4 + 1 + 32 + 4 + body0_len;
        let mut bytes = bytes;
        bytes[this_hash_offset + 5] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let err = chain.verify().unwrap_err();
        assert!(matches!(err, ChainError::Tamper { .. }));
    }

    #[test]
    fn reopen_continues_the_chain() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.chain");
        {
            let chain = AuditChain::open(&path).unwrap();
            chain.append(&entry("alice", "a")).unwrap();
        }
        let chain = AuditChain::open(&path).unwrap();
        // Last hash loaded from disk; next append chains from it.
        let last_before = chain.last_hash();
        assert_ne!(last_before, GENESIS_HASH);
        chain.append(&entry("bob", "b")).unwrap();
        let recs = chain.verify().unwrap();
        assert_eq!(recs.len(), 2);
    }

    #[test]
    fn oversize_body_rejected() {
        let dir = tempdir().unwrap();
        let chain = AuditChain::open(dir.path().join("audit.chain")).unwrap();
        let big = ChainedAuditEntry {
            timestamp_ms: 0,
            principal_id: "x".into(),
            sql: "y".repeat(MAX_BODY_BYTES + 1),
            outcome: ChainedOutcome::Ok,
        };
        assert!(matches!(chain.append(&big), Err(ChainError::RecordTooLarge)));
    }
}
