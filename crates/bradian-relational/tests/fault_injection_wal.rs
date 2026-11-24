//! ADR 0252 — WAL fault injection primitive + RowStore
//! recovery assertions.

use bradian_relational::fault_injection::{
    file_len, flip_byte_at, truncate_file_to, zero_range,
};
use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn file_len_matches_metadata() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"hello").unwrap();
    assert_eq!(file_len(&path).unwrap(), 5);
}

#[test]
fn truncate_shrinks_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"hello world").unwrap();
    truncate_file_to(&path, 5).unwrap();
    assert_eq!(file_len(&path).unwrap(), 5);
    assert_eq!(std::fs::read(&path).unwrap(), b"hello");
}

#[test]
fn truncate_cannot_grow() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"hi").unwrap();
    assert!(truncate_file_to(&path, 10).is_err());
}

#[test]
fn flip_byte_round_trips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"abc").unwrap();
    flip_byte_at(&path, 1).unwrap();
    assert_ne!(std::fs::read(&path).unwrap(), b"abc");
    flip_byte_at(&path, 1).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"abc");
}

#[test]
fn flip_byte_past_end_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"abc").unwrap();
    assert!(flip_byte_at(&path, 3).is_err());
    assert!(flip_byte_at(&path, 99).is_err());
}

#[test]
fn zero_range_fills_region() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"hello world").unwrap();
    // Zero bytes [2..7) → 'llo w'. Trailing region is 'orld'.
    zero_range(&path, 2, 5).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..2], b"he");
    assert_eq!(&bytes[2..7], &[0u8; 5]);
    assert_eq!(&bytes[7..], b"orld");
}

#[test]
fn zero_range_past_end_errors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f");
    std::fs::write(&path, b"abc").unwrap();
    assert!(zero_range(&path, 2, 5).is_err());
}

/// Build a RowStore and write `n` keys. Returns the path to
/// `rows.wal` after close so callers can mutate it before
/// reopening.
fn seeded_wal(dir: &std::path::Path, n: usize) -> std::path::PathBuf {
    let store = RowStore::open(dir).unwrap();
    for i in 0..n {
        let k = format!("k{i:03}");
        let v = format!("v{i:03}");
        store.put(k.as_bytes(), v.as_bytes()).unwrap();
    }
    drop(store);
    dir.join("rows.wal")
}

#[test]
fn clean_reopen_matches_committed_state() {
    let dir = tempdir().unwrap();
    let _wal = seeded_wal(dir.path(), 5);
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..5 {
        let k = format!("k{i:03}");
        let v = format!("v{i:03}");
        assert_eq!(store.get(k.as_bytes()).as_deref(), Some(v.as_bytes()));
    }
}

#[test]
fn flipping_a_middle_wal_byte_is_detected_on_reopen() {
    // A bit flip in the body of a committed frame breaks the
    // frame CRC. RowStore must NOT silently accept the
    // corrupted data — it must surface an error.
    let dir = tempdir().unwrap();
    let wal = seeded_wal(dir.path(), 5);
    let len = file_len(&wal).unwrap();
    // Flip a byte roughly in the middle of the WAL, which
    // lands inside one of the committed frame bodies.
    let offset = len / 2;
    flip_byte_at(&wal, offset).unwrap();
    let result = RowStore::open(dir.path());
    assert!(result.is_err(), "flipped-byte WAL must not open cleanly");
    let err = result.err().unwrap();
    assert!(
        matches!(err, StoreError::Corruption(_) | StoreError::Io(_)),
        "expected Corruption or Io, got {err:?}",
    );
}

#[test]
fn truncating_wal_tail_is_handled_as_torn_frame() {
    // Chopping bytes off the WAL tail simulates a power-loss
    // mid-append. RowStore's recovery MUST either (a) drop
    // the torn frame and reopen cleanly with the earlier
    // committed prefix, OR (b) refuse to open with a clear
    // error. It must never silently expose partial data.
    let dir = tempdir().unwrap();
    let wal = seeded_wal(dir.path(), 5);
    let len = file_len(&wal).unwrap();
    // Chop one byte.
    truncate_file_to(&wal, len - 1).unwrap();
    match RowStore::open(dir.path()) {
        Ok(store) => {
            // First four keys MUST still be readable — only the
            // last torn frame may be lost.
            for i in 0..4 {
                let k = format!("k{i:03}");
                assert!(store.get(k.as_bytes()).is_some(),
                    "earlier committed prefix lost after tail truncation at i={i}");
            }
        }
        Err(err) => {
            assert!(
                matches!(err, StoreError::Corruption(_) | StoreError::Io(_)),
                "expected Corruption or Io, got {err:?}",
            );
        }
    }
}

#[test]
fn zero_wal_magic_is_rejected() {
    // Zeroing the WAL magic bytes should be caught on open —
    // the file exists but doesn't look like a BRWL file.
    let dir = tempdir().unwrap();
    let wal = seeded_wal(dir.path(), 3);
    zero_range(&wal, 0, 4).unwrap();
    let result = RowStore::open(dir.path());
    assert!(result.is_err(), "zeroed magic must not open cleanly");
}
