//! ADR 0104 — Native audit log foundation.
//!
//! Append-only persistent log of (timestamp, principal_id,
//! sql_text, outcome). Used by `SqlEngine::execute_as_audited`
//! so every authz-gated statement leaves a trace.
//!
//! **Honest scope.** This is NOT tamper-proof — the file is a
//! plain CRC32-per-record append-only log on local disk. BRD-
//! SEC-005 production audit requires:
//!   - cryptographic chaining (hash-linked records);
//!   - externally anchored integrity proofs (signed digests);
//!   - immutable storage (WORM / S3 object lock);
//!   - access-controlled retention.
//!
//! None of those are shipped here. What IS shipped:
//!   - Deterministic record format with per-record CRC32.
//!   - `AuditLog::append(entry)` + fsync.
//!   - `AuditLog::read_all()` for operator inspection.
//!   - Rejection of records above a reasonable max size so a
//!     runaway SQL string cannot blow up the file.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crc32fast::hash;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAGIC: &[u8; 4] = b"BAUD";
const FORMAT_VERSION: u8 = 1;
const MAX_SQL_BYTES: usize = 64 * 1024;
const MAX_PRINCIPAL_BYTES: usize = 256;
const MAX_ERROR_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    pub timestamp_ms: i64,
    pub principal_id: String,
    pub sql: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Ok,
    Err(String),
    AuthzDenied(String),
}

#[derive(Debug, Error)]
pub enum AuditError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("record exceeds max size")]
    RecordTooLarge,
    #[error("corruption: {0}")]
    Corruption(String),
    #[error("serialization: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, AuditError>;

/// Append-only audit log.
#[derive(Debug, Clone)]
pub struct AuditLog {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug)]
struct Inner {
    file: File,
    path: PathBuf,
}

impl AuditLog {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().read(true).create(true)
            .append(true).open(&path)?;
        Ok(Self { inner: Arc::new(Mutex::new(Inner { file, path })) })
    }

    /// Append + fsync one entry.
    pub fn append(&self, entry: &AuditEntry) -> Result<()> {
        if entry.sql.len() > MAX_SQL_BYTES
            || entry.principal_id.len() > MAX_PRINCIPAL_BYTES
        {
            return Err(AuditError::RecordTooLarge);
        }
        if let Outcome::Err(m) | Outcome::AuthzDenied(m) = &entry.outcome {
            if m.len() > MAX_ERROR_BYTES {
                return Err(AuditError::RecordTooLarge);
            }
        }
        let body = serde_json::to_vec(entry)?;
        let body_len = u32::try_from(body.len())
            .map_err(|_| AuditError::RecordTooLarge)?;
        let crc = hash(&body);
        let mut rec = Vec::with_capacity(4 + 1 + 4 + body.len() + 4);
        rec.extend_from_slice(MAGIC);
        rec.push(FORMAT_VERSION);
        rec.extend_from_slice(&body_len.to_le_bytes());
        rec.extend_from_slice(&body);
        rec.extend_from_slice(&crc.to_le_bytes());

        let mut inner = self.inner.lock();
        inner.file.write_all(&rec)?;
        inner.file.sync_all()?;
        Ok(())
    }

    /// Replay the whole log. Returns records in append order.
    /// A torn record at the tail (short header or body) is
    /// treated as end-of-log — recovery stops there.
    pub fn read_all(&self) -> Result<Vec<AuditEntry>> {
        let inner = self.inner.lock();
        let mut file = File::open(&inner.path)?;
        file.seek(SeekFrom::Start(0))?;
        let mut out = Vec::new();
        loop {
            let mut header = [0u8; 9];
            match file.read_exact(&mut header) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(AuditError::Io(e)),
            }
            if &header[0..4] != MAGIC {
                return Err(AuditError::Corruption(
                    "audit record bad magic".into(),
                ));
            }
            if header[4] != FORMAT_VERSION {
                return Err(AuditError::Corruption(format!(
                    "audit record unsupported version {}", header[4],
                )));
            }
            let body_len = u32::from_le_bytes(header[5..9].try_into().unwrap()) as usize;
            let mut body = vec![0u8; body_len];
            if file.read_exact(&mut body).is_err() { break; }
            let mut crc_buf = [0u8; 4];
            if file.read_exact(&mut crc_buf).is_err() { break; }
            let declared = u32::from_le_bytes(crc_buf);
            if hash(&body) != declared {
                return Err(AuditError::Corruption(
                    "audit record crc mismatch".into(),
                ));
            }
            let entry: AuditEntry = serde_json::from_slice(&body)?;
            out.push(entry);
        }
        Ok(out)
    }

    pub fn path(&self) -> PathBuf {
        self.inner.lock().path.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn entry(ts: i64, who: &str, sql: &str, outcome: Outcome) -> AuditEntry {
        AuditEntry { timestamp_ms: ts, principal_id: who.into(),
            sql: sql.into(), outcome }
    }

    #[test]
    fn append_and_read_round_trip() {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("audit.log")).unwrap();
        log.append(&entry(1, "root", "SELECT 1", Outcome::Ok)).unwrap();
        log.append(&entry(2, "analyst", "DROP TABLE x",
            Outcome::AuthzDenied("needs Ddl".into()))).unwrap();
        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].principal_id, "root");
        assert_eq!(all[0].outcome, Outcome::Ok);
        assert!(matches!(all[1].outcome, Outcome::AuthzDenied(_)));
    }

    #[test]
    fn oversize_sql_rejected_without_writing() {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("audit.log")).unwrap();
        let big = "x".repeat(MAX_SQL_BYTES + 1);
        let e = log.append(&entry(1, "root", &big, Outcome::Ok));
        assert!(matches!(e, Err(AuditError::RecordTooLarge)));
        assert_eq!(log.read_all().unwrap().len(), 0,
            "rejected record must not be written");
    }

    #[test]
    fn torn_tail_is_treated_as_end_of_log() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::open(&path).unwrap();
        log.append(&entry(1, "root", "SELECT 1", Outcome::Ok)).unwrap();
        // Append garbage that looks like a partial header.
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&[0xFF, 0xFF]).unwrap();
            f.sync_all().unwrap();
        }
        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 1, "valid first record readable; torn tail ignored");
    }

    #[test]
    fn crc_tamper_detected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("audit.log");
        let log = AuditLog::open(&path).unwrap();
        log.append(&entry(1, "root", "SELECT 1", Outcome::Ok)).unwrap();
        // Flip a body byte.
        let mut bytes = std::fs::read(&path).unwrap();
        let body_start = 9; // after header
        bytes[body_start + 1] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        let err = log.read_all().unwrap_err();
        assert!(matches!(err, AuditError::Corruption(_)));
    }

    #[test]
    fn concurrent_appends_serialize_through_the_inner_mutex() {
        let dir = tempdir().unwrap();
        let log = AuditLog::open(dir.path().join("audit.log")).unwrap();
        let handles: Vec<_> = (0..10).map(|i| {
            let l = log.clone();
            std::thread::spawn(move || {
                l.append(&entry(i, &format!("u{i}"), "SELECT 1", Outcome::Ok)).unwrap();
            })
        }).collect();
        for h in handles { h.join().unwrap(); }
        let all = log.read_all().unwrap();
        assert_eq!(all.len(), 10);
    }
}
