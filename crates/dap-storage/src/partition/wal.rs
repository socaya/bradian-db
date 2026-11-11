//! Append-only partition WAL with CRC32 per record.
//!
//! Replay stops on the first corrupt or truncated record and returns the last good LSN.
//! Truncation is handled by the partition layer once a checkpoint is durably published.

use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

use crate::checksum::crc32;

use super::delta::{PrimaryKey, Row};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum WalRecord {
    Begin {
        lsn: u64,
        started_at_ms: i64,
    },
    Upsert {
        lsn: u64,
        key: PrimaryKey,
        source_version: u64,
        row: Row,
    },
    Delete {
        lsn: u64,
        key: PrimaryKey,
        source_version: u64,
    },
    Checkpoint {
        lsn: u64,
        visible_generation: u64,
        watermark_ms: i64,
    },
    Commit {
        lsn: u64,
        tx_id: u64,
    },
}

impl WalRecord {
    pub fn lsn(&self) -> u64 {
        match self {
            WalRecord::Begin { lsn, .. }
            | WalRecord::Upsert { lsn, .. }
            | WalRecord::Delete { lsn, .. }
            | WalRecord::Checkpoint { lsn, .. }
            | WalRecord::Commit { lsn, .. } => *lsn,
        }
    }
}

#[derive(Debug)]
pub struct WalWriter {
    file: File,
    path: PathBuf,
}

impl WalWriter {
    pub fn open(path: &Path) -> DapResult<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(path)?;
        Ok(Self {
            file,
            path: path.to_path_buf(),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn append(&mut self, record: &WalRecord) -> DapResult<()> {
        let payload = bincode::serialize(record)
            .map_err(|e| DapError::Serialization(e.to_string()))?;
        let payload_len = payload.len() as u32;
        let payload_crc = crc32(&payload);

        self.file.write_u32::<LittleEndian>(payload_len)?;
        self.file.write_u32::<LittleEndian>(payload_crc)?;
        self.file.write_all(&payload)?;
        Ok(())
    }

    pub fn sync(&self) -> DapResult<()> {
        self.file.sync_all()?;
        Ok(())
    }

    pub fn append_and_sync(&mut self, record: &WalRecord) -> DapResult<()> {
        self.append(record)?;
        self.sync()
    }

    /// Discard the WAL. Used after a successful checkpoint drives
    /// `wal_truncation_lsn` past every current record. The caller is responsible for
    /// sequencing truncation after the manifest has been durably updated.
    pub fn truncate(&mut self) -> DapResult<()> {
        self.file.set_len(0)?;
        self.file.sync_all()?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct WalReader {
    path: PathBuf,
}

impl WalReader {
    pub fn open(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }

    /// Replay the WAL from the beginning. Stops at the first CRC mismatch or truncated
    /// record and returns the records read so far. Returns `(records, stopped_at_truncation)`.
    pub fn replay(&self) -> DapResult<Vec<WalRecord>> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&self.path)?;
        let mut reader = BufReader::new(file);
        let mut out = Vec::new();

        loop {
            let payload_len = match reader.read_u32::<LittleEndian>() {
                Ok(v) => v as usize,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(DapError::from(e)),
            };
            let payload_crc = match reader.read_u32::<LittleEndian>() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Truncated record: stop at the last good one.
                    break;
                }
                Err(e) => return Err(DapError::from(e)),
            };
            let mut payload = vec![0u8; payload_len];
            match reader.read_exact(&mut payload) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    // Truncated payload: stop.
                    break;
                }
                Err(e) => return Err(DapError::from(e)),
            }
            if crc32(&payload) != payload_crc {
                // Corruption: stop here with what we have.
                break;
            }
            let record: WalRecord = bincode::deserialize(&payload)
                .map_err(|e| DapError::corruption(format!("wal decode: {e}")))?;
            out.push(record);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::Value;
    use tempfile::tempdir;

    fn pk(v: i64) -> PrimaryKey {
        PrimaryKey(vec![Value::I64(v)])
    }

    #[test]
    fn append_and_replay_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wal.log");

        {
            let mut w = WalWriter::open(&path).unwrap();
            w.append_and_sync(&WalRecord::Begin { lsn: 1, started_at_ms: 0 })
                .unwrap();
            w.append_and_sync(&WalRecord::Upsert {
                lsn: 2,
                key: pk(10),
                source_version: 1,
                row: Row(vec![Value::I64(42)]),
            })
            .unwrap();
            w.append_and_sync(&WalRecord::Delete {
                lsn: 3,
                key: pk(10),
                source_version: 2,
            })
            .unwrap();
            w.append_and_sync(&WalRecord::Commit { lsn: 4, tx_id: 1 })
                .unwrap();
        }

        let r = WalReader::open(&path);
        let recs = r.replay().unwrap();
        assert_eq!(recs.len(), 4);
        assert_eq!(recs[0].lsn(), 1);
        assert_eq!(recs[1].lsn(), 2);
        assert_eq!(recs[2].lsn(), 3);
        assert_eq!(recs[3].lsn(), 4);
    }

    #[test]
    fn truncated_payload_stops_replay_without_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wal.log");
        {
            let mut w = WalWriter::open(&path).unwrap();
            w.append_and_sync(&WalRecord::Begin { lsn: 1, started_at_ms: 0 })
                .unwrap();
            w.append_and_sync(&WalRecord::Commit { lsn: 2, tx_id: 1 })
                .unwrap();
        }
        // Chop off the last 3 bytes.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(bytes.len() - 3);
        std::fs::write(&path, &bytes).unwrap();

        let recs = WalReader::open(&path).replay().unwrap();
        assert_eq!(recs.len(), 1, "stopped at truncated second record");
    }

    #[test]
    fn corrupt_crc_stops_replay() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("wal.log");
        {
            let mut w = WalWriter::open(&path).unwrap();
            w.append_and_sync(&WalRecord::Begin { lsn: 1, started_at_ms: 0 })
                .unwrap();
            w.append_and_sync(&WalRecord::Commit { lsn: 2, tx_id: 1 })
                .unwrap();
        }
        // Flip a byte inside the second record's payload (after the 8-byte length+crc prefix
        // of the first record and then its payload). Easier: flip the very last byte.
        let mut bytes = std::fs::read(&path).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();

        let recs = WalReader::open(&path).replay().unwrap();
        assert_eq!(recs.len(), 1, "stopped at corrupt second record");
    }
}
