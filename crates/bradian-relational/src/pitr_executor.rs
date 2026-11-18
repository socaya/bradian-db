//! ADR 0167 — PITR restore executor.
//!
//! Applies a `RestorePlan` from ADR 0132 to a destination
//! directory: copies the base-backup contents + replays
//! each delta file.
//!
//! Delta on-disk format matches ADR 0121's writer:
//!
//! ```text
//! +--------+--------+----------+----------+---------+
//! | MAGIC  | ver    | base_lsn | tip_lsn  | body_len|  CRC32
//! | BRDL-DLT| u16   | u64      | u64      | u32     |  u32
//! +--------+--------+----------+----------+---------+
//! | body_len bytes (opaque, caller interprets)
//! ```
//!
//! Honest scope — base-backup directory is copied via
//! `fs::copy`; delta body bytes are appended to a
//! caller-named "wal.replay" file so downstream engines
//! can consume. The executor does NOT:
//!   - Validate base backup internals (callers verify
//!     receipts via BACE first).
//!   - Parse delta bodies into WAL frames (the body is
//!     opaque — the caller knows how to replay).
//!   - Truncate the final WAL tail to `target_lsn`
//!     exactly; the planner caps `target_lsn` at a
//!     delta tip, so no mid-delta truncation is
//!     needed in the current contract.
//!
//! Out of scope:
//! - Online restore (process is offline).
//! - Backup integrity re-verification.
//! - Parallel delta copy.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::pitr_planner::RestorePlan;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("destination {path:?} already exists; refuse to overwrite")]
    DestinationExists { path: PathBuf },
    #[error("base backup {id} missing from source directory")]
    MissingBase { id: String },
    #[error("delta {id} missing from source directory")]
    MissingDelta { id: String },
    #[error("delta {id} malformed: {reason}")]
    MalformedDelta { id: String, reason: String },
}

pub type ExecutorResult<T> = std::result::Result<T, ExecutorError>;

/// ADR 0167 — apply a `RestorePlan` under `source_dir`
/// to a fresh `destination` directory.
///
/// Expectations about the source layout:
/// - Each `BaseBackup.id` is a directory under
///   `source_dir/bases/<id>` that will be recursively
///   copied into `destination`.
/// - Each `DeltaBackup.id` is a file at
///   `source_dir/deltas/<id>.brdl`; bodies are appended
///   into `destination/wal.replay` in plan order.
///
/// Returns the total bytes appended to `wal.replay`.
pub fn apply_plan(
    plan: &RestorePlan,
    source_dir: &Path,
    destination: &Path,
) -> ExecutorResult<u64> {
    if destination.exists() {
        return Err(ExecutorError::DestinationExists {
            path: destination.to_path_buf(),
        });
    }
    fs::create_dir_all(destination)?;

    // Copy the base directory into destination.
    let base_dir = source_dir.join("bases").join(&plan.base.id);
    if !base_dir.exists() {
        return Err(ExecutorError::MissingBase {
            id: plan.base.id.clone(),
        });
    }
    copy_dir_recursive(&base_dir, destination)?;

    // Append each delta's body to wal.replay in plan order.
    let mut replay = File::create(destination.join("wal.replay"))?;
    let mut total_bytes = 0u64;
    for delta in &plan.deltas {
        let path = source_dir.join("deltas").join(format!("{}.brdl", delta.id));
        if !path.exists() {
            return Err(ExecutorError::MissingDelta { id: delta.id.clone() });
        }
        let body = read_delta_body(&path, &delta.id)?;
        replay.write_all(&body)?;
        total_bytes += body.len() as u64;
    }
    replay.sync_all()?;
    Ok(total_bytes)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> ExecutorResult<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn read_delta_body(path: &Path, id: &str) -> ExecutorResult<Vec<u8>> {
    const MAGIC: &[u8; 8] = b"BRDL-DLT";
    const HEADER: usize = 8 + 2 + 8 + 8 + 4;  // magic + ver + base + tip + body_len
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if buf.len() < HEADER + 4 {
        return Err(ExecutorError::MalformedDelta {
            id: id.into(),
            reason: format!("truncated: {} bytes < header + crc", buf.len()),
        });
    }
    if &buf[0..8] != MAGIC {
        return Err(ExecutorError::MalformedDelta {
            id: id.into(),
            reason: "bad magic".into(),
        });
    }
    let body_len = u32::from_be_bytes(buf[26..30].try_into().unwrap()) as usize;
    if buf.len() < HEADER + body_len + 4 {
        return Err(ExecutorError::MalformedDelta {
            id: id.into(),
            reason: format!("body short: header says {body_len} bytes"),
        });
    }
    let body = buf[HEADER..HEADER + body_len].to_vec();
    // CRC check (ADR 0121)
    let actual_crc = u32::from_be_bytes(
        buf[HEADER + body_len..HEADER + body_len + 4].try_into().unwrap());
    let expected_crc = crc32fast::hash(&buf[0..HEADER + body_len]);
    if actual_crc != expected_crc {
        return Err(ExecutorError::MalformedDelta {
            id: id.into(),
            reason: format!("CRC mismatch: expected {expected_crc:#010x}, got {actual_crc:#010x}"),
        });
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitr_planner::{BaseBackup, DeltaBackup};
    use tempfile::tempdir;

    fn write_fake_delta(path: &Path, base: u64, tip: u64, body: &[u8]) {
        const MAGIC: &[u8; 8] = b"BRDL-DLT";
        let mut buf = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&base.to_be_bytes());
        buf.extend_from_slice(&tip.to_be_bytes());
        buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
        buf.extend_from_slice(body);
        let crc = crc32fast::hash(&buf);
        buf.extend_from_slice(&crc.to_be_bytes());
        std::fs::write(path, &buf).unwrap();
    }

    fn make_source() -> (tempfile::TempDir, PathBuf) {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        fs::create_dir_all(root.join("bases").join("base1")).unwrap();
        std::fs::write(root.join("bases").join("base1").join("checkpoint.bin"),
            b"BASE-CONTENT").unwrap();
        fs::create_dir_all(root.join("deltas")).unwrap();
        write_fake_delta(&root.join("deltas").join("d1.brdl"), 100, 200, b"DELTA-ONE");
        write_fake_delta(&root.join("deltas").join("d2.brdl"), 200, 300, b"DELTA-TWO");
        (dir, root)
    }

    #[test]
    fn apply_plan_copies_base_and_appends_deltas() {
        let (_src, src_root) = make_source();
        let dest = tempdir().unwrap();
        let dest_root = dest.path().join("restored");
        let plan = RestorePlan {
            base: BaseBackup { id: "base1".into(), tip_lsn: 100 },
            deltas: vec![
                DeltaBackup { id: "d1".into(), base_lsn: 100, tip_lsn: 200 },
                DeltaBackup { id: "d2".into(), base_lsn: 200, tip_lsn: 300 },
            ],
            target_lsn: 300,
        };
        let bytes = apply_plan(&plan, &src_root, &dest_root).unwrap();
        assert_eq!(bytes, 9 + 9);  // "DELTA-ONE" + "DELTA-TWO"
        // Base file copied.
        assert_eq!(
            std::fs::read(dest_root.join("checkpoint.bin")).unwrap(),
            b"BASE-CONTENT"
        );
        // wal.replay contains both delta bodies in order.
        let replay = std::fs::read(dest_root.join("wal.replay")).unwrap();
        assert_eq!(replay, b"DELTA-ONEDELTA-TWO");
    }

    #[test]
    fn apply_plan_rejects_existing_destination() {
        let (_src, src_root) = make_source();
        let dest = tempdir().unwrap();
        let plan = RestorePlan {
            base: BaseBackup { id: "base1".into(), tip_lsn: 100 },
            deltas: vec![],
            target_lsn: 100,
        };
        let err = apply_plan(&plan, &src_root, dest.path()).unwrap_err();
        assert!(matches!(err, ExecutorError::DestinationExists { .. }));
    }

    #[test]
    fn apply_plan_rejects_missing_base() {
        let (_src, src_root) = make_source();
        let dest = tempdir().unwrap();
        let plan = RestorePlan {
            base: BaseBackup { id: "ghost".into(), tip_lsn: 0 },
            deltas: vec![],
            target_lsn: 0,
        };
        let err = apply_plan(&plan, &src_root, &dest.path().join("x")).unwrap_err();
        assert!(matches!(err, ExecutorError::MissingBase { .. }));
    }

    #[test]
    fn apply_plan_rejects_missing_delta() {
        let (_src, src_root) = make_source();
        let dest = tempdir().unwrap();
        let plan = RestorePlan {
            base: BaseBackup { id: "base1".into(), tip_lsn: 100 },
            deltas: vec![DeltaBackup {
                id: "nonexistent".into(), base_lsn: 100, tip_lsn: 150,
            }],
            target_lsn: 150,
        };
        let err = apply_plan(&plan, &src_root, &dest.path().join("x")).unwrap_err();
        assert!(matches!(err, ExecutorError::MissingDelta { .. }));
    }

    #[test]
    fn apply_plan_detects_corrupted_delta() {
        let (_src, src_root) = make_source();
        // Flip a byte in d1.
        let path = src_root.join("deltas").join("d1.brdl");
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[40] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        let dest = tempdir().unwrap();
        let plan = RestorePlan {
            base: BaseBackup { id: "base1".into(), tip_lsn: 100 },
            deltas: vec![DeltaBackup { id: "d1".into(), base_lsn: 100, tip_lsn: 200 }],
            target_lsn: 200,
        };
        let err = apply_plan(&plan, &src_root, &dest.path().join("x")).unwrap_err();
        assert!(matches!(err, ExecutorError::MalformedDelta { .. }));
    }

    #[test]
    fn empty_delta_chain_only_copies_base() {
        let (_src, src_root) = make_source();
        let dest = tempdir().unwrap();
        let dest_root = dest.path().join("restored");
        let plan = RestorePlan {
            base: BaseBackup { id: "base1".into(), tip_lsn: 100 },
            deltas: vec![],
            target_lsn: 100,
        };
        let bytes = apply_plan(&plan, &src_root, &dest_root).unwrap();
        assert_eq!(bytes, 0);
        // wal.replay still created (empty).
        assert!(dest_root.join("wal.replay").exists());
    }
}
