//! End-to-end consistent backup and restore against actual Bradian WAL files.

use std::fs;
use std::sync::Arc;
use std::thread;

use bradian_relational::backup::BackupReceipt;
use bradian_relational::row_store::{RowMutation, RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn complete_backup_and_restore_roundtrip_with_checkpoint_and_wal() {
    let base = tempdir().unwrap();
    let db = base.path().join("primary");
    let backup = base.path().join("backup-20261112");
    let restored = base.path().join("restored");
    let report;
    {
        let store = RowStore::open(&db).unwrap();
        store.put(b"alice", b"active").unwrap();
        store.put(b"bob", b"inactive").unwrap();
        store.commit_batch(None, &[
            RowMutation::Put { key: b"charlie".to_vec(), value: b"new".to_vec() },
            RowMutation::Delete { key: b"bob".to_vec() },
        ]).unwrap();
        report = store.backup_to(&backup).unwrap();
        assert_eq!(report.rows, 2);
        assert_eq!(report.last_lsn, 3);
        assert_eq!(report.checkpoint_lsn, 3);
        assert!(backup.join("backup.info").is_file());
        assert!(backup.join("checkpoint.current").is_file());
        assert!(backup.join("snapshot-3.brp").is_file());
        assert!(backup.join("index-3.bri").is_file());
        assert!(backup.join("rows.wal").is_file());
        assert!(!backup.join("snapshot-3.tmp").exists());
    }
    let result = RowStore::restore_backup(&backup, &restored).unwrap();
    assert_eq!(result, report);
    let recovered = RowStore::open(&restored).unwrap();
    assert_eq!(recovered.last_lsn(), 3);
    assert_eq!(recovered.get(b"alice"), Some(b"active".to_vec()));
    assert_eq!(recovered.get(b"bob"), None);
    assert_eq!(recovered.get(b"charlie"), Some(b"new".to_vec()));
    assert_eq!(recovered.verify_checkpoint_index().unwrap().rows, 2);
    recovered.put(b"later", b"restored-write").unwrap();
    assert_eq!(recovered.last_lsn(), 4);
}

#[test]
fn source_changes_after_backup_do_not_change_captured_state() {
    let base = tempdir().unwrap();
    let source = base.path().join("db");
    let backup = base.path().join("frozen");
    let restored = base.path().join("restore");
    let store = RowStore::open(&source).unwrap();
    store.put(b"key", b"before").unwrap();
    let receipt = store.backup_to(&backup).unwrap();
    store.put(b"key", b"after").unwrap();
    store.put(b"new", b"not-in-backup").unwrap();
    RowStore::restore_backup(&backup, &restored).unwrap();
    let recovered = RowStore::open(&restored).unwrap();
    assert_eq!(recovered.last_lsn(), receipt.last_lsn);
    assert_eq!(recovered.get(b"key"), Some(b"before".to_vec()));
    assert_eq!(recovered.get(b"new"), None);
}

#[test]
fn backup_preserves_checkpoint_plus_subsequent_wal_frames() {
    let base = tempdir().unwrap();
    let root = base.path().join("source");
    let backup = base.path().join("captured");
    let restored = base.path().join("restored");
    let store = RowStore::open(&root).unwrap();
    store.put(b"base", b"first").unwrap();
    store.checkpoint().unwrap();
    store.put(b"late", b"after-checkpoint").unwrap();
    store.put(b"extra", b"also-in-WAL").unwrap();
    let report = store.backup_current_generation(&backup).unwrap();
    assert_eq!(report.checkpoint_lsn, 1);
    assert_eq!(report.last_lsn, 3);
    assert_eq!(report.rows, 3);
    assert!(fs::metadata(backup.join("rows.wal")).unwrap().len() > 0);
    RowStore::restore_backup(&backup, &restored).unwrap();
    let recovered = RowStore::open(&restored).unwrap();
    assert_eq!(recovered.get(b"base"), Some(b"first".to_vec()));
    assert_eq!(recovered.get(b"late"), Some(b"after-checkpoint".to_vec()));
    assert_eq!(recovered.get(b"extra"), Some(b"also-in-WAL".to_vec()));
    assert_eq!(recovered.last_lsn(), 3);
}

#[test]
fn restoring_refuses_existing_destination_and_keeps_it_unchanged() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let existing = base.path().join("existing");
    let store = RowStore::open(&db).unwrap();
    store.put(b"data", b"original").unwrap();
    store.backup_to(&backup).unwrap();
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("sentinel"), b"must-stay").unwrap();
    assert!(RowStore::restore_backup(&backup, &existing).is_err());
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"must-stay");
    assert!(store.backup_to(&backup).is_err());
}

#[test]
fn corrupted_backup_manifest_or_wal_cannot_be_restored() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"data", b"present").unwrap();
    store.backup_to(&backup).unwrap();

    let mut descriptor = fs::read(backup.join("backup.info")).unwrap();
    descriptor[12] ^= 0x23;
    fs::write(backup.join("backup.info"), descriptor).unwrap();
    assert!(RowStore::restore_backup(&backup, &base.path().join("invalid")).is_err());
}

#[test]
fn corrupted_snapshot_backup_fails_even_when_descriptor_is_valid() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"data", b"present").unwrap();
    store.backup_to(&backup).unwrap();
    let snapshot = backup.join("snapshot-1.brp");
    let mut bytes = fs::read(&snapshot).unwrap();
    bytes[4096 + 55] ^= 0xff;
    fs::write(snapshot, bytes).unwrap();
    let restored = base.path().join("restore");
    assert!(RowStore::restore_backup(&backup, &restored).is_err());
    assert!(!restored.exists(), "no verified directory should be published");
}

#[test]
fn metadata_format_roundtrip_rejects_lsn_regressions() {
    let receipt = BackupReceipt { checkpoint_lsn: 21, last_lsn: 42, rows: 8 };
    let bytes = receipt.encode();
    assert_eq!(bytes.len(), 40);
    assert_eq!(BackupReceipt::decode(&bytes).unwrap(), receipt);
    let invalid = BackupReceipt { checkpoint_lsn: 9, last_lsn: 8, rows: 1 };
    assert!(BackupReceipt::decode(&invalid.encode()).is_err());
}

#[test]
fn backup_directory_inside_active_database_is_rejected() {
    let base = tempdir().unwrap();
    let root = base.path().join("db");
    let store = RowStore::open(&root).unwrap();
    store.put(b"row", b"safe").unwrap();
    assert!(matches!(
        store.backup_to(root.join("nested-backup")),
        Err(StoreError::Corruption(_))
    ));
    assert_eq!(store.get(b"row"), Some(b"safe".to_vec()));
}

#[test]
fn verified_backup_with_parallel_writers_contains_one_committed_state() {
    let base = tempdir().unwrap();
    let source = Arc::new(RowStore::open(base.path().join("source")).unwrap());
    for i in 0..50 {
        source.put(format!("row-{i:03}").as_bytes(), b"initial").unwrap();
    }
    let writer = {
        let store = Arc::clone(&source);
        thread::spawn(move || {
            for i in 0..30 {
                store.put(format!("row-{i:03}").as_bytes(), b"updated").unwrap();
            }
        })
    };
    let backup = base.path().join("consistent");
    let report = source.backup_to(&backup).unwrap();
    writer.join().unwrap();
    let restored = base.path().join("restored");
    RowStore::restore_backup(&backup, &restored).unwrap();
    let copy = RowStore::open(&restored).unwrap();
    assert_eq!(copy.last_lsn(), report.last_lsn);
    assert_eq!(copy.len(), 50);
    // Writer modifies rows in key order; a consistent backup must contain
    // a prefix of the updates, never a partial WAL record or corrupt row.
    let updated = copy.scan().into_iter().filter(|(_,v)|v == b"updated").count();
    assert!(updated <= 30);
    for i in 0..updated {
        assert_eq!(copy.get(format!("row-{i:03}").as_bytes()), Some(b"updated".to_vec()));
    }
}

#[test]
fn corrupted_post_checkpoint_wal_is_rejected_during_restore() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"base", b"checkpointed").unwrap();
    store.checkpoint().unwrap();
    store.put(b"after", b"wal-only").unwrap();
    store.backup_current_generation(&backup).unwrap();

    let path = backup.join("rows.wal");
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(path, bytes).unwrap();

    let restored = base.path().join("restored");
    assert!(RowStore::restore_backup(&backup, &restored).is_err());
    assert!(!restored.exists());
}

#[test]
fn current_generation_backup_requires_committed_checkpoint() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let store = RowStore::open(&db).unwrap();
    store.put(b"key", b"value").unwrap();
    let dest = base.path().join("backup");
    assert!(store.backup_current_generation(&dest).is_err());
    assert!(!dest.exists());
    assert_eq!(store.last_lsn(), 1);
}
