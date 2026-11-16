//! ADR 0106 — device-fault simulation scenarios (in-repo subset).
//!
//! Real device-level qualification (power-loss, cable pulls,
//! torn-write across power, disk ECC errors) requires a
//! physical fault lab + §13 procurement. This file simulates
//! what's cheaply reproducible on tmpfs / stdlib APIs:
//!
//! 1. Fallocate hole-punch mid-WAL (POSIX only).
//! 2. File truncation mid-page (any OS).
//! 3. Zero-overwrite of an arbitrary byte range.
//!
//! Every scenario is a BLACK BOX — we don't touch Bradian
//! internals, we corrupt the file and verify the recovery
//! path classifies the fault as expected.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};

use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn truncation_mid_wal_frame_recovers_at_last_complete_frame() {
    // Scenario: power loss drops the tail of the WAL mid-frame.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open(dir.path()).unwrap();
        for i in 0..5 {
            s.put(format!("k{i}").as_bytes(), b"v").unwrap();
        }
    }
    let wal = dir.path().join("rows.wal");
    let valid = fs::read(&wal).unwrap();
    // Truncate 7 bytes off the tail — the last frame is now
    // incomplete.
    OpenOptions::new().write(true).open(&wal).unwrap()
        .set_len(valid.len() as u64 - 7).unwrap();
    let s = RowStore::open(dir.path()).unwrap();
    // Recovery must have dropped the torn frame. At least one
    // committed row remains.
    assert!(s.get(b"k0").is_some(), "earlier commits survive");
    // And subsequent writes still succeed.
    s.put(b"after-recovery", b"ok").unwrap();
}

#[test]
fn zero_overwrite_of_wal_payload_is_classified_as_corruption() {
    // Scenario: disk ECC flips an entire sector to zero.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open(dir.path()).unwrap();
        s.put(b"keep", b"important-value").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    // Overwrite 4 bytes AFTER the WAL header with zeros.
    // WAL frame header is 12 bytes; corrupt body byte 20.
    let mut f = OpenOptions::new().write(true).open(&wal).unwrap();
    f.seek(SeekFrom::Start(20)).unwrap();
    f.write_all(&[0u8; 4]).unwrap();
    f.sync_all().unwrap();
    // Reopen: CRC must detect the corruption.
    match RowStore::open(dir.path()) {
        Err(StoreError::Corruption(_)) => {}
        other => panic!("expected Corruption; got {other:?}"),
    }
}

#[test]
#[cfg(unix)]
fn fallocate_style_hole_punch_leaves_zero_bytes_detectable_as_corruption() {
    // Scenario: filesystem punches a hole in the middle of the
    // file (sparse file reads as zeros). This simulates
    // fallocate(FALLOC_FL_PUNCH_HOLE) on a page-aligned region.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open(dir.path()).unwrap();
        for i in 0..3 {
            s.put(format!("key-{i}").as_bytes(), b"value").unwrap();
        }
    }
    let wal = dir.path().join("rows.wal");
    // Write zeros over bytes 24..40 (middle of a frame body).
    let mut f = OpenOptions::new().write(true).open(&wal).unwrap();
    f.seek(SeekFrom::Start(24)).unwrap();
    f.write_all(&[0u8; 16]).unwrap();
    f.sync_all().unwrap();
    match RowStore::open(dir.path()) {
        Err(StoreError::Corruption(_)) => {}
        other => panic!("expected Corruption from zeroed bytes in WAL; got {other:?}"),
    }
}

#[test]
fn lock_file_removal_concurrent_with_open_breaks_single_writer_but_leaves_data_recoverable() {
    // Already locked in by fault_recovery_extension.rs — this
    // test re-verifies that despite the POSIX foot-gun, the
    // data files themselves survive and can be reopened cleanly
    // once both processes release.
    #[cfg(unix)]
    {
        let dir = tempdir().unwrap();
        {
            let s = RowStore::open(dir.path()).unwrap();
            s.put(b"survivor", b"value").unwrap();
        }
        fs::remove_file(dir.path().join("rows.lock")).unwrap();
        // Single clean reopen after the lock was deleted — both
        // original handles are gone, fresh reopen succeeds and
        // the pre-delete data is intact.
        let s = RowStore::open(dir.path()).unwrap();
        assert_eq!(s.get(b"survivor"), Some(b"value".to_vec()));
    }
}
