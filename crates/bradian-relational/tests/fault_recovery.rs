//! Deterministic row-store fault recovery (ADR 0052).
//!
//! Each test simulates a crash outcome by direct filesystem manipulation between
//! `RowStore` drops. These exercise the real recovery path on real files; no
//! fault-injection abstraction is introduced into production code. They do not
//! claim real power-loss or cross-platform device qualification.

use std::fs::{self, OpenOptions};
use std::io::Write;

use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

fn open(path: &std::path::Path) -> RowStore {
    RowStore::open(path).unwrap()
}

#[test]
fn zero_padded_wal_extension_beyond_last_frame_fails_closed() {
    // Outcome: a `write_all` under ENOSPC can leave the file grown with
    // trailing NUL bytes if the kernel extended the file before erroring. The
    // WAL magic check must refuse a zero-magic header rather than treating it
    // as "an incomplete header, trim it".
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"committed", b"value").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        // Twelve zero bytes = complete WAL_HEADER made entirely of NULs.
        file.write_all(&[0_u8; 12]).unwrap();
        file.sync_all().unwrap();
    }
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Corruption(_))));
    // The original bytes are untouched by the refused open.
    let after = fs::read(&wal).unwrap();
    assert_eq!(&after[after.len() - 12..], &[0_u8; 12]);
}

#[test]
fn wal_header_with_zero_filled_body_fails_crc_rather_than_trimming() {
    // Outcome: a complete header with a declared non-zero body, whose body
    // bytes were never written to storage and read back as zeros. The header
    // itself passes magic + length bounds, so recovery must enforce CRC rather
    // than accepting the all-zero body as "incomplete tail".
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"committed", b"value").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(b"BRWL").unwrap();
        file.write_all(&32_u32.to_le_bytes()).unwrap(); // body length in valid range
        file.write_all(&0_u32.to_le_bytes()).unwrap(); // CRC = 0, body = all zeros
        file.write_all(&[0_u8; 32]).unwrap();
        file.sync_all().unwrap();
    }
    match RowStore::open(dir.path()) {
        Err(StoreError::Corruption(msg)) => {
            assert!(
                msg.starts_with("WAL CRC mismatch") || msg.starts_with("WAL sequence"),
                "unexpected corruption message: {msg}",
            );
        }
        other => panic!("expected Corruption, got {other:?}"),
    }
}

#[test]
fn orphan_snapshot_and_index_tmp_artifacts_do_not_shadow_the_committed_generation() {
    // Outcome: a prior checkpoint attempt crashed after writing both
    // snapshot-<N>.tmp and index-<N>.tmp but before renaming to the published
    // .brp/.bri names and publishing the manifest. The next open must use the
    // committed generation referenced by `checkpoint.current`.
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"anchor", b"original").unwrap();
        store.checkpoint().unwrap(); // publishes generation 1
        store.put(b"after", b"wal").unwrap();
    }
    // Simulate an interrupted later checkpoint at LSN 5 that never published.
    fs::write(dir.path().join("snapshot-5.tmp"), b"interrupted-snapshot").unwrap();
    fs::write(dir.path().join("index-5.tmp"), b"interrupted-index").unwrap();

    let store = open(dir.path());
    assert_eq!(store.get(b"anchor"), Some(b"original".to_vec()));
    assert_eq!(store.get(b"after"), Some(b"wal".to_vec()));
    // Artifacts are not deleted by a passive open; they are not part of state.
    assert!(dir.path().join("snapshot-5.tmp").exists());
    assert!(dir.path().join("index-5.tmp").exists());
    // The .tmp files were NOT mistaken for a newer authoritative generation.
    assert_eq!(fs::read(dir.path().join("snapshot-1.brp")).unwrap().len() % 4096, 0);
}

#[test]
fn multiple_aborted_checkpoint_generations_do_not_corrupt_recovery() {
    // Two separate aborted checkpoints left .tmp artifacts at different LSNs
    // above the committed generation. Recovery must still use the committed
    // snapshot and reject neither of the two .tmp artifacts as authoritative.
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"id", b"v1").unwrap();
        store.checkpoint().unwrap(); // commits generation 1
    }
    fs::write(dir.path().join("snapshot-3.tmp"), b"earlier-attempt").unwrap();
    fs::write(dir.path().join("index-3.tmp"), b"earlier-attempt").unwrap();
    fs::write(dir.path().join("snapshot-7.tmp"), b"later-attempt").unwrap();
    fs::write(dir.path().join("index-7.tmp"), b"later-attempt").unwrap();

    let store = open(dir.path());
    assert_eq!(store.get(b"id"), Some(b"v1".to_vec()));
    // A fresh checkpoint still succeeds without touching the orphan .tmp files.
    store.put(b"next", b"v1").unwrap();
    let new_lsn = store.checkpoint().unwrap();
    assert!(new_lsn > 1);
    for lsn in [3_u64, 7] {
        assert!(dir.path().join(format!("snapshot-{lsn}.tmp")).exists());
        assert!(dir.path().join(format!("index-{lsn}.tmp")).exists());
    }
}

#[test]
fn stray_incidental_files_in_store_directory_do_not_break_recovery() {
    // Operators or system utilities can leave files like .DS_Store, README or
    // ad-hoc backups in the store directory. These must not break the
    // ordinary reopen path or the pre-manifest discovery path.
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"a", b"v").unwrap();
        store.checkpoint().unwrap();
    }
    fs::write(dir.path().join(".DS_Store"), b"macos-metadata").unwrap();
    fs::write(dir.path().join("README"), b"operator note").unwrap();
    fs::write(dir.path().join("snapshot-backup.bak"), b"ad-hoc copy").unwrap();

    // Ordinary reopen path (manifest present): unaffected by incidental files.
    {
        let store = open(dir.path());
        assert_eq!(store.get(b"a"), Some(b"v".to_vec()));
    }

    // Pre-manifest path (manifest absent): the incidental filenames do not match
    // `snapshot-<lsn>.brp`, so they are skipped during discovery.
    fs::remove_file(dir.path().join("checkpoint.current")).unwrap();
    let store = open(dir.path());
    assert_eq!(store.get(b"a"), Some(b"v".to_vec()));
}

#[test]
fn pending_manifest_leftover_never_replaces_current_even_with_matching_lsn() {
    // A pending manifest file surviving a crash must never be promoted to
    // authoritative without going through `publish`, even if its encoded
    // contents parse. This complements the existing "garbage pending" test
    // with a decodable-but-unpublished pending file.
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"k", b"v").unwrap();
        store.checkpoint().unwrap();
    }
    let current_bytes = fs::read(dir.path().join("checkpoint.current")).unwrap();
    // The pending copy is byte-identical: it is still not the published marker.
    fs::write(dir.path().join("checkpoint.pending"), &current_bytes).unwrap();
    let store = open(dir.path());
    assert_eq!(store.get(b"k"), Some(b"v".to_vec()));
    // A subsequent checkpoint publishes a new authoritative marker; the
    // previously-stranded pending file does not interfere.
    store.put(b"next", b"v").unwrap();
    store.checkpoint().unwrap();
    assert_eq!(
        fs::read(dir.path().join("checkpoint.current")).unwrap().len(),
        current_bytes.len(),
    );
}

#[test]
fn index_file_missing_after_commit_rebuilds_without_corrupting_snapshot() {
    // Outcome: the atomic manifest publication succeeded but a subsequent
    // `index-<lsn>.bri` write was lost (deleted by hand, or on a filesystem
    // that sync'd the manifest but not the index). Recovery must detect the
    // missing index and rebuild it from the authoritative snapshot pages
    // without corrupting the committed snapshot bytes.
    let dir = tempdir().unwrap();
    let snapshot_bytes;
    {
        let store = open(dir.path());
        store.put(b"a", b"v").unwrap();
        store.put(b"b", b"v").unwrap();
        let lsn = store.checkpoint().unwrap();
        snapshot_bytes = fs::read(dir.path().join(format!("snapshot-{lsn}.brp"))).unwrap();
        drop(store);
        fs::remove_file(dir.path().join(format!("index-{lsn}.bri"))).unwrap();
    }
    let store = open(dir.path());
    // Reads still succeed and return exactly the committed values.
    assert_eq!(store.get(b"a"), Some(b"v".to_vec()));
    assert_eq!(store.get(b"b"), Some(b"v".to_vec()));
    // The snapshot file was not rewritten during recovery; find the current
    // generation by scanning the directory for the single published .brp file.
    let brp: Vec<_> = fs::read_dir(dir.path()).unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".brp"))
        .collect();
    assert_eq!(brp.len(), 1, "exactly one committed generation must remain");
    assert_eq!(fs::read(brp[0].path()).unwrap(), snapshot_bytes);
}

#[test]
fn backup_descriptor_missing_but_payload_present_is_detected_as_incomplete() {
    // Outcome: a backup was interrupted between copying the payload files
    // and writing `backup.info`. The archive directory contains the
    // snapshot/index/wal but no descriptor. Restore must refuse rather
    // than replaying an unlabelled archive.
    use bradian_relational::row_store::RowStore;
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let archive = dir.path().join("archive");
    {
        let store = open(&source);
        store.put(b"k", b"v").unwrap();
        store.checkpoint().unwrap();
        store.backup_current_generation(&archive).unwrap();
    }
    // Delete the descriptor but keep the payload.
    fs::remove_file(archive.join("backup.info")).unwrap();
    let restored = dir.path().join("restored");
    assert!(RowStore::restore_backup(&archive, &restored).is_err());
    assert!(!restored.exists(), "incomplete archive must not publish a destination");
}

#[test]
fn catalog_journal_truncated_mid_event_fails_closed_without_partial_catalog() {
    // Outcome: the WAL under PersistentCatalog was truncated mid-frame.
    // Catalog open must refuse rather than publishing only the events
    // that happen to decode.
    use bradian_relational::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
    let dir = tempdir().unwrap();
    {
        let catalog = PersistentCatalog::open(dir.path()).unwrap();
        catalog.create_database("one").unwrap();
        catalog.create_database("two").unwrap();
    }
    let wal_path = dir.path().join("rows.wal");
    let mut bytes = fs::read(&wal_path).unwrap();
    // Chop off the last few bytes of the final WAL frame.
    assert!(bytes.len() > 16);
    bytes.truncate(bytes.len() - 3);
    fs::write(&wal_path, bytes).unwrap();
    // The RowStore under the catalog trims an incomplete tail during
    // recovery, so the catalog itself may open; but if the trimmed state
    // produces fewer events than the original acknowledged LSN, replay
    // detects the mismatch. We assert the outcome is either (a) a clean
    // open at exactly one database, or (b) a Corruption error. In no
    // case is a partial multi-event catalog published.
    match PersistentCatalog::open(dir.path()) {
        Ok(catalog) => {
            let count = catalog.snapshot().databases().count();
            assert!(count == 1, "trimmed catalog must not expose partial state; got {count} databases");
        }
        Err(PersistentCatalogError::Corruption(_))
        | Err(PersistentCatalogError::Storage(_))
        | Err(PersistentCatalogError::Resource(_)) => {}
        Err(other) => panic!("unexpected error after WAL truncation: {other:?}"),
    }
}

#[test]
fn wal_frame_with_length_zero_fails_closed() {
    // Outcome: after a complete frame, a subsequent header declares a body
    // length of 0. The length-bound check requires 13 <= len <= MAX_WAL_BODY,
    // so 0 must refuse as Corruption rather than being treated as an
    // incomplete/empty body.
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"committed", b"value").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(b"BRWL").unwrap();
        file.write_all(&0_u32.to_le_bytes()).unwrap(); // declared body length = 0
        file.write_all(&0_u32.to_le_bytes()).unwrap(); // CRC
        file.sync_all().unwrap();
    }
    match RowStore::open(dir.path()) {
        Err(StoreError::Corruption(msg)) => {
            assert!(
                msg.contains("invalid WAL frame length"),
                "unexpected corruption message: {msg}",
            );
        }
        other => panic!("expected invalid WAL frame length Corruption, got {other:?}"),
    }
}

#[test]
fn wal_frame_with_length_above_maximum_fails_closed() {
    // Outcome: after a complete frame, a subsequent header declares a body
    // length above MAX_WAL_BODY (1 MiB). The length-bound check must refuse
    // before any attempt to allocate or read the body — a malicious or
    // corrupted header must not be able to force a huge allocation.
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"committed", b"value").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(b"BRWL").unwrap();
        // 1 MiB + 1 byte is exactly one past MAX_WAL_BODY.
        file.write_all(&((1024 * 1024 + 1) as u32).to_le_bytes()).unwrap();
        file.write_all(&0_u32.to_le_bytes()).unwrap(); // CRC
        file.sync_all().unwrap();
    }
    match RowStore::open(dir.path()) {
        Err(StoreError::Corruption(msg)) => {
            assert!(
                msg.contains("invalid WAL frame length"),
                "unexpected corruption message: {msg}",
            );
        }
        other => panic!("expected invalid WAL frame length Corruption, got {other:?}"),
    }
}

#[cfg(unix)]
#[test]
fn deleting_the_rows_lock_path_while_fd_is_held_keeps_the_store_live() {
    // Outcome (Unix-specific): the rows.lock file is held via an advisory
    // lock on an open file descriptor. If the path is unlinked while the
    // fd is still held, the fd's lock survives — writes through the live
    // RowStore keep working, and a second process attempting to open the
    // directory gets its own fresh inode without colliding with the
    // original lock. This documents the real behaviour; it is not a
    // recommendation to unlink the lock file in production.
    let dir = tempdir().unwrap();
    let store = open(dir.path());
    store.put(b"alice", b"first").unwrap();
    // Unlink the lock path. The original fd inside `store` is unaffected.
    fs::remove_file(dir.path().join("rows.lock")).unwrap();
    // The live store keeps working — the advisory lock is on the fd, not the path.
    store.put(b"bob", b"second").unwrap();
    assert_eq!(store.get(b"alice"), Some(b"first".to_vec()));
    assert_eq!(store.get(b"bob"), Some(b"second".to_vec()));
    // A fresh open installs a new lock file at the same path. On Unix this
    // succeeds because it creates a new inode; the original store still
    // holds its own fd on the original (now path-less) inode.
    //
    // Depending on the platform, this may or may not succeed. We do not
    // assert exclusivity under path manipulation — the production invariant
    // is only that the live store retains correct behaviour.
    drop(store);
    // After drop, the directory may reopen cleanly (the fresh lock file
    // created by the second open will stay). Confirm the previously
    // committed rows survive the drop.
    let reopened = open(dir.path());
    assert_eq!(reopened.get(b"alice"), Some(b"first".to_vec()));
    assert_eq!(reopened.get(b"bob"), Some(b"second".to_vec()));
}
