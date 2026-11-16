//! ADR 0121 — Incremental backup (delta since last full).
//!
//! BRD-DR-001: operators currently must take a full backup
//! each time. This module ships a delta primitive: given a
//! base LSN, emit only the WAL frames newer than that LSN.
//! A delta manifest records (base_lsn, tip_lsn, frame_count,
//! crc32) so restore can combine FULL + N × DELTA in LSN
//! order.
//!
//! Honest scope: WAL-frame level only. Checkpoint pages are
//! not re-captured in a delta; restore requires the base
//! full backup for them. Operators must retain the base
//! backup until a new full is taken.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"BRDL-DLT";
const FORMAT_VERSION: u8 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeltaManifest {
    pub base_lsn: u64,
    pub tip_lsn: u64,
    pub frame_count: u64,
    pub bytes: u64,
    pub created_at_ms: i64,
}

#[derive(Debug, Error)]
pub enum DeltaError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("corruption: {0}")]
    Corruption(String),
    #[error("delta would be empty: no frames newer than base_lsn {base_lsn}")]
    Empty { base_lsn: u64 },
    #[error("base_lsn {requested} exceeds current tip {tip}; cannot produce delta")]
    BaseBeyondTip { requested: u64, tip: u64 },
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, DeltaError>;

/// Scan `source_wal` for frames with LSN > base_lsn and copy
/// them to `dest_dir` as `delta-<base>-<tip>.wal` alongside a
/// `delta.manifest` JSON.
pub fn write_delta(
    source_wal: &Path,
    dest_dir: &Path,
    base_lsn: u64,
) -> Result<DeltaManifest> {
    std::fs::create_dir_all(dest_dir)?;
    let mut src = File::open(source_wal)?;
    src.seek(SeekFrom::Start(0))?;

    let mut tip_lsn = base_lsn;
    let mut frame_count = 0u64;
    let mut bytes = 0u64;
    let mut delta_bytes: Vec<u8> = Vec::new();

    loop {
        let start = src.stream_position()?;
        let mut header = [0u8; 12];
        match src.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(DeltaError::Io(e)),
        }
        if &header[0..4] != b"BRWL" {
            return Err(DeltaError::Corruption(format!(
                "bad WAL magic at byte {start}",
            )));
        }
        let body_len = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if !(8..=16 * 1024 * 1024).contains(&body_len) {
            return Err(DeltaError::Corruption(format!(
                "implausible WAL body_len {body_len} at byte {start}",
            )));
        }
        let mut body = vec![0u8; body_len];
        if src.read_exact(&mut body).is_err() { break; }
        let lsn = u64::from_le_bytes(body[0..8].try_into().unwrap());
        if lsn > base_lsn {
            delta_bytes.extend_from_slice(&header);
            delta_bytes.extend_from_slice(&body);
            frame_count += 1;
            bytes += (header.len() + body.len()) as u64;
            if lsn > tip_lsn { tip_lsn = lsn; }
        }
    }

    if frame_count == 0 {
        return Err(DeltaError::Empty { base_lsn });
    }
    if tip_lsn < base_lsn {
        return Err(DeltaError::BaseBeyondTip {
            requested: base_lsn, tip: tip_lsn,
        });
    }

    // Write delta file: magic + version + base_lsn + tip_lsn + body + crc32.
    let delta_path = dest_dir.join(format!("delta-{base_lsn}-{tip_lsn}.wal"));
    let mut out = Vec::with_capacity(8 + 1 + 16 + delta_bytes.len() + 4);
    out.extend_from_slice(MAGIC);
    out.push(FORMAT_VERSION);
    out.extend_from_slice(&base_lsn.to_le_bytes());
    out.extend_from_slice(&tip_lsn.to_le_bytes());
    out.extend_from_slice(&delta_bytes);
    let crc = crc32fast::hash(&out);
    out.extend_from_slice(&crc.to_le_bytes());

    {
        let mut f = File::create(&delta_path)?;
        f.write_all(&out)?;
        f.sync_all()?;
    }

    let manifest = DeltaManifest {
        base_lsn, tip_lsn, frame_count, bytes,
        created_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64).unwrap_or(0),
    };
    let manifest_path = dest_dir.join("delta.manifest");
    {
        let mut f = OpenOptions::new().write(true).create(true).truncate(true)
            .open(&manifest_path)?;
        f.write_all(&serde_json::to_vec_pretty(&manifest)?)?;
        f.sync_all()?;
    }

    Ok(manifest)
}

/// Read a delta file produced by `write_delta`. Verifies CRC
/// and returns the extracted (base, tip, frame_bytes).
pub fn read_delta(path: &Path) -> Result<(u64, u64, Vec<u8>)> {
    let bytes = std::fs::read(path)?;
    if bytes.len() < 8 + 1 + 16 + 4 {
        return Err(DeltaError::Corruption("delta file too short".into()));
    }
    if &bytes[..8] != MAGIC {
        return Err(DeltaError::Corruption("delta magic mismatch".into()));
    }
    if bytes[8] != FORMAT_VERSION {
        return Err(DeltaError::Corruption(format!(
            "unsupported delta version {}", bytes[8],
        )));
    }
    let base_lsn = u64::from_le_bytes(bytes[9..17].try_into().unwrap());
    let tip_lsn = u64::from_le_bytes(bytes[17..25].try_into().unwrap());
    let body_end = bytes.len() - 4;
    let declared = u32::from_le_bytes(bytes[body_end..].try_into().unwrap());
    let actual = crc32fast::hash(&bytes[..body_end]);
    if declared != actual {
        return Err(DeltaError::Corruption("delta crc mismatch".into()));
    }
    Ok((base_lsn, tip_lsn, bytes[25..body_end].to_vec()))
}

/// Path layout helper.
pub fn delta_file_path(dest_dir: &Path, base_lsn: u64, tip_lsn: u64) -> PathBuf {
    dest_dir.join(format!("delta-{base_lsn}-{tip_lsn}.wal"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::row_store::RowStore;
    use tempfile::tempdir;

    #[test]
    fn write_delta_captures_only_frames_above_base() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("src");
        {
            let s = RowStore::open(&db).unwrap();
            s.put(b"a", b"1").unwrap();  // lsn 1
            s.put(b"b", b"2").unwrap();  // lsn 2
            s.put(b"c", b"3").unwrap();  // lsn 3
        }
        let delta_dir = dir.path().join("delta");
        let manifest = write_delta(&db.join("rows.wal"), &delta_dir, 1).unwrap();
        // base=1 so we expect 2 frames (lsn 2 and 3).
        assert_eq!(manifest.base_lsn, 1);
        assert_eq!(manifest.tip_lsn, 3);
        assert_eq!(manifest.frame_count, 2);
        assert!(manifest.bytes > 0);
        assert!(delta_dir.join("delta-1-3.wal").is_file());
        assert!(delta_dir.join("delta.manifest").is_file());
    }

    #[test]
    fn empty_delta_rejected() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("src");
        {
            let s = RowStore::open(&db).unwrap();
            s.put(b"k", b"v").unwrap();   // lsn 1
        }
        let err = write_delta(&db.join("rows.wal"), &dir.path().join("d"), 999)
            .unwrap_err();
        assert!(matches!(err, DeltaError::Empty { base_lsn: 999 }));
    }

    #[test]
    fn read_delta_round_trip() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("src");
        {
            let s = RowStore::open(&db).unwrap();
            for i in 1..=5 {
                s.put(format!("k{i}").as_bytes(), b"v").unwrap();
            }
        }
        let delta_dir = dir.path().join("delta");
        write_delta(&db.join("rows.wal"), &delta_dir, 2).unwrap();
        let (base, tip, frames) = read_delta(&delta_file_path(&delta_dir, 2, 5)).unwrap();
        assert_eq!(base, 2);
        assert_eq!(tip, 5);
        assert!(!frames.is_empty());
    }

    #[test]
    fn crc_tamper_rejected() {
        let dir = tempdir().unwrap();
        let db = dir.path().join("src");
        {
            let s = RowStore::open(&db).unwrap();
            s.put(b"k", b"v").unwrap();
        }
        let delta_dir = dir.path().join("delta");
        write_delta(&db.join("rows.wal"), &delta_dir, 0).unwrap();
        let path = delta_file_path(&delta_dir, 0, 1);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[30] ^= 0xFF;
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_delta(&path).is_err());
    }
}
