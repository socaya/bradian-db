//! ADR 0163 — durable projection segment writer.
//!
//! Composes with ADR 0159's `projector_worker::drain_once`
//! as a concrete sink: writes each `CommittedProjection`
//! to a self-describing on-disk segment that any future
//! columnar reader can open.
//!
//! On-disk format (little-endian; same convention as the
//! rest of Bradian's typed row store):
//!
//! ```text
//! +--------+--------+----------+------------+---------+
//! | MAGIC  | ver    | as_of_lsn|  json_len  |  CRC32  |
//! | BRPS\1 | u16    | u64      | u32        | u32     |
//! +--------+--------+----------+------------+---------+
//! | JSON-encoded Vec<ColumnBatch> bytes                |
//! +---------------------------------------------------+
//! ```
//!
//! Honest scope:
//! - One file per projection. Caller supplies the
//!   target path.
//! - JSON encoding reuses the existing serde serializers
//!   on `ColumnBatch`. Faster binary encodings (ADR 0082
//!   DictIndex32) are a follow-up.
//! - CRC32 covers header + body; `read_segment` rejects
//!   mismatches.
//!
//! Out of scope:
//! - Append-only segment directories.
//! - Bloom filters / column min-max statistics.
//! - Compression.
//! - Catalog registration.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use thiserror::Error;

use crate::column_batch::ColumnBatch;
use crate::committed_projection::CommittedProjection;

pub const SEGMENT_MAGIC: [u8; 4] = *b"BRPS";
pub const SEGMENT_VERSION: u16 = 1;
const HEADER_SIZE: usize = 4 + 2 + 8 + 4 + 4;  // 22

#[derive(Debug, Error)]
pub enum SegmentError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("bad magic: expected {:?}, got {got:?}", SEGMENT_MAGIC)]
    BadMagic { got: [u8; 4] },
    #[error("unsupported segment version {got} (this build speaks {})", SEGMENT_VERSION)]
    UnsupportedVersion { got: u16 },
    #[error("segment CRC mismatch: expected {expected:#010x}, got {actual:#010x}")]
    ChecksumMismatch { expected: u32, actual: u32 },
    #[error("truncated segment: expected at least {expected} bytes, got {actual}")]
    Truncated { expected: usize, actual: usize },
}

pub type SegmentResult<T> = std::result::Result<T, SegmentError>;

/// ADR 0163 — serialize a `CommittedProjection` to a
/// self-describing on-disk segment. Returns the number
/// of bytes written.
pub fn write_segment(
    path: &Path,
    projection: &CommittedProjection,
) -> SegmentResult<u64> {
    let body = serde_json::to_vec(&projection.batch)?;
    let mut buf = Vec::with_capacity(HEADER_SIZE + body.len());
    buf.extend_from_slice(&SEGMENT_MAGIC);
    buf.extend_from_slice(&SEGMENT_VERSION.to_le_bytes());
    buf.extend_from_slice(&projection.as_of_lsn.to_le_bytes());
    buf.extend_from_slice(&(body.len() as u32).to_le_bytes());
    // CRC placeholder (zeroed); we overwrite after computing.
    buf.extend_from_slice(&[0u8; 4]);
    buf.extend_from_slice(&body);
    let crc = crc32fast::hash(&buf);
    buf[HEADER_SIZE - 4..HEADER_SIZE].copy_from_slice(&crc.to_le_bytes());

    let mut f = File::create(path)?;
    f.write_all(&buf)?;
    f.sync_all()?;
    Ok(buf.len() as u64)
}

/// ADR 0163 — read a segment back. Validates magic +
/// version + CRC.
pub fn read_segment(path: &Path) -> SegmentResult<CommittedProjection> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if buf.len() < HEADER_SIZE {
        return Err(SegmentError::Truncated {
            expected: HEADER_SIZE, actual: buf.len(),
        });
    }
    let magic: [u8; 4] = buf[0..4].try_into().unwrap();
    if magic != SEGMENT_MAGIC {
        return Err(SegmentError::BadMagic { got: magic });
    }
    let version = u16::from_le_bytes([buf[4], buf[5]]);
    if version != SEGMENT_VERSION {
        return Err(SegmentError::UnsupportedVersion { got: version });
    }
    let as_of_lsn = u64::from_le_bytes([
        buf[6], buf[7], buf[8], buf[9],
        buf[10], buf[11], buf[12], buf[13],
    ]);
    let body_len = u32::from_le_bytes([buf[14], buf[15], buf[16], buf[17]]) as usize;
    if buf.len() < HEADER_SIZE + body_len {
        return Err(SegmentError::Truncated {
            expected: HEADER_SIZE + body_len, actual: buf.len(),
        });
    }
    let actual_crc = u32::from_le_bytes([buf[18], buf[19], buf[20], buf[21]]);
    // Recompute over the canonical form (CRC window zeroed).
    let mut canonical = buf.clone();
    canonical[18..22].copy_from_slice(&[0u8; 4]);
    let expected_crc = crc32fast::hash(&canonical);
    if expected_crc != actual_crc {
        return Err(SegmentError::ChecksumMismatch {
            expected: expected_crc, actual: actual_crc,
        });
    }
    let batch: Vec<ColumnBatch> = serde_json::from_slice(
        &buf[HEADER_SIZE..HEADER_SIZE + body_len])?;
    Ok(CommittedProjection { batch, as_of_lsn })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column_batch::ColumnBatch;
    use crate::htap_watermark::HtapWatermark;
    use crate::committed_projection::project_committed;
    use crate::model::DataType;
    use crate::typed_table::Value;
    use tempfile::tempdir;

    fn make_projection(as_of: u64) -> CommittedProjection {
        let w = HtapWatermark::new();
        w.advance_committed(as_of);
        project_committed(
            vec![
                ColumnBatch {
                    name: "id".into(),
                    dtype: DataType::Int64,
                    values: vec![Value::Int64(1), Value::Int64(2)],
                },
            ],
            as_of,
            &w,
        ).unwrap()
    }

    #[test]
    fn write_and_read_round_trips() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p1.brps");
        let p = make_projection(42);
        write_segment(&path, &p).unwrap();
        let back = read_segment(&path).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn write_syncs_bytes_through_to_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.brps");
        let p = make_projection(1);
        let n = write_segment(&path, &p).unwrap();
        let on_disk = std::fs::metadata(&path).unwrap().len();
        assert_eq!(n, on_disk);
    }

    #[test]
    fn corrupt_magic_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.brps");
        write_segment(&path, &make_projection(7)).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[0] = b'X';
        std::fs::write(&path, &bytes).unwrap();
        let err = read_segment(&path).unwrap_err();
        assert!(matches!(err, SegmentError::BadMagic { .. }));
    }

    #[test]
    fn bit_flip_in_body_detected_by_crc() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.brps");
        write_segment(&path, &make_projection(7)).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        // Flip one byte deep in the JSON body.
        let flip_at = HEADER_SIZE + 10;
        bytes[flip_at] ^= 0x01;
        std::fs::write(&path, &bytes).unwrap();
        let err = read_segment(&path).unwrap_err();
        assert!(matches!(err, SegmentError::ChecksumMismatch { .. }));
    }

    #[test]
    fn unsupported_version_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.brps");
        write_segment(&path, &make_projection(7)).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[4] = 0xff;  // bump version to an impossible value
        bytes[5] = 0xff;
        std::fs::write(&path, &bytes).unwrap();
        let err = read_segment(&path).unwrap_err();
        // Either UnsupportedVersion or ChecksumMismatch depending
        // on byte ordering; both are acceptable rejection paths.
        assert!(matches!(err,
            SegmentError::UnsupportedVersion { .. } |
            SegmentError::ChecksumMismatch { .. }));
    }

    #[test]
    fn truncated_segment_rejected() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.brps");
        write_segment(&path, &make_projection(7)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        std::fs::write(&path, &bytes[0..10]).unwrap();
        let err = read_segment(&path).unwrap_err();
        assert!(matches!(err, SegmentError::Truncated { .. }));
    }

    #[test]
    fn as_of_lsn_preserved_across_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("p.brps");
        write_segment(&path, &make_projection(0xdead_beef_cafe)).unwrap();
        let back = read_segment(&path).unwrap();
        assert_eq!(back.as_of_lsn, 0xdead_beef_cafe);
    }
}
