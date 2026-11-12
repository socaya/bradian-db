//! Crash-boundary recovery and full verified-index audit against BRPG snapshot.

use std::fs;

use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn restart_reconstructs_absent_index_using_authoritative_snapshot() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        for i in 0..225 {
            store.put(format!("key-{i:04}").as_bytes(), format!("value-{i}").as_bytes()).unwrap();
        }
        store.checkpoint().unwrap();
        assert_eq!(store.verify_checkpoint_index().unwrap().rows, 225);
    }
    let index = dir.path().join("index-225.bri");
    fs::remove_file(&index).unwrap(); // emulate crash before index publication
    assert!(!index.exists());
    let store = RowStore::open(dir.path()).unwrap();
    assert!(index.exists());
    let report = store.verify_checkpoint_index().unwrap();
    assert_eq!(report.checkpoint_lsn, 225);
    assert_eq!(report.rows, 225);
    assert!(report.data_pages >= 1);
    assert_eq!(store.indexed_get(b"key-0224").unwrap(), Some(b"value-224".to_vec()));
    assert_eq!(store.indexed_range(b"key-0110", b"key-0120").unwrap().len(), 10);
}

#[test]
fn crash_during_index_temp_file_does_not_poison_published_snapshot() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"committed", b"safe").unwrap();
        store.checkpoint().unwrap();
    }
    fs::remove_file(dir.path().join("index-1.bri")).unwrap();
    fs::write(dir.path().join("index-1.rebuild.tmp"), b"incomplete").unwrap();
    fs::write(dir.path().join("index-1.tmp"), b"also-incomplete").unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(store.get(b"committed"), Some(b"safe".to_vec()));
    assert_eq!(store.indexed_get(b"committed").unwrap(), Some(b"safe".to_vec()));
    assert!(dir.path().join("index-1.bri").exists());
}

#[test]
fn auto_rebuild_replays_post_checkpoint_wal_and_delta() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"alpha", b"checkpoint-value").unwrap();
        store.put(b"beta", b"remove-me").unwrap();
        store.checkpoint().unwrap();
        store.put(b"alpha", b"new-value").unwrap();
        store.delete(b"beta").unwrap();
        store.put(b"gamma", b"added").unwrap();
    }
    fs::remove_file(dir.path().join("index-2.bri")).unwrap();
    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.last_lsn(), 5);
    assert_eq!(reopened.uncheckpointed_index_changes(), 3);
    assert_eq!(reopened.indexed_get(b"alpha").unwrap(), Some(b"new-value".to_vec()));
    assert_eq!(reopened.indexed_get(b"beta").unwrap(), None);
    assert_eq!(reopened.indexed_get(b"gamma").unwrap(), Some(b"added".to_vec()));
    assert_eq!(reopened.indexed_range(b"a", b"z").unwrap(), vec![
        (b"alpha".to_vec(), b"new-value".to_vec()),
        (b"gamma".to_vec(), b"added".to_vec()),
    ]);
    assert_eq!(reopened.verify_checkpoint_index().unwrap().rows, 2);
}

#[test]
fn corrupted_authoritative_snapshot_is_not_repaired_as_if_it_were_an_index() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"row", b"data").unwrap();
        store.checkpoint().unwrap();
    }
    fs::remove_file(dir.path().join("index-1.bri")).unwrap();
    let snapshot = dir.path().join("snapshot-1.brp");
    let mut bytes = fs::read(&snapshot).unwrap();
    bytes[4096 + 32] ^= 1;
    fs::write(snapshot, bytes).unwrap();
    assert!(RowStore::open(dir.path()).is_err());
    assert!(!dir.path().join("index-1.bri").exists());
}

#[test]
fn damaged_existing_published_index_is_not_silently_overwritten() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"row", b"data").unwrap();
        store.checkpoint().unwrap();
    }
    let index = dir.path().join("index-1.bri");
    let mut damaged = fs::read(&index).unwrap();
    damaged[4096 + 37] ^= 0x5a;
    fs::write(&index, &damaged).unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    assert!(store.verify_checkpoint_index().is_err());
    assert!(store.rebuild_missing_checkpoint_index().is_err());
    assert_eq!(fs::read(index).unwrap(), damaged);
    assert_eq!(store.get(b"row"), Some(b"data".to_vec()));
}

#[test]
fn recovery_missing_index_is_idempotent_and_never_overwrites_existing() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"row", b"data").unwrap();
        store.checkpoint().unwrap();
        let report = store.rebuild_missing_checkpoint_index().unwrap();
        assert!(!report.rebuilt);
    }
    let index = dir.path().join("index-1.bri");
    let original = fs::read(&index).unwrap();
    fs::remove_file(&index).unwrap();
    // Reopening owns the directory lock and reconstructs only the missing
    // derived index, without changing the authoritative checkpoint.
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(fs::read(&index).unwrap(), original);
    let again = store.rebuild_missing_checkpoint_index().unwrap();
    assert!(!again.rebuilt);
    assert!(!store.verify_checkpoint_index().unwrap().rebuilt);
}

#[test]
fn empty_checkpoint_has_rebuildable_zero_entry_index() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        assert_eq!(store.checkpoint().unwrap(), 0);
    }
    fs::remove_file(dir.path().join("index-0.bri")).unwrap();
    let reopened = RowStore::open(dir.path()).unwrap();
    let report = reopened.verify_checkpoint_index().unwrap();
    assert_eq!(report.rows, 0);
    assert_eq!(report.data_pages, 0);
    assert_eq!(reopened.indexed_get(b"unknown").unwrap(), None);
}

#[test]
fn absent_snapshot_cannot_be_used_as_an_index_recovery_source() {
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    assert!(matches!(store.verify_checkpoint_index(), Err(StoreError::Corruption(_))));
    assert!(matches!(store.rebuild_missing_checkpoint_index(), Err(StoreError::Corruption(_))));
}


#[test]
fn integrity_audit_detects_valid_crc_with_wrong_physical_row_pointer() {
    use bradian_relational::page::{Page, PAGE_SIZE};

    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"first", b"one").unwrap();
        store.put(b"second", b"two").unwrap();
        store.checkpoint().unwrap();
    }
    let path = dir.path().join("index-2.bri");
    let mut bytes = fs::read(&path).unwrap();
    let leaf_offset = PAGE_SIZE;
    let page = Page::decode(&bytes[leaf_offset..leaf_offset + PAGE_SIZE]).unwrap();
    let mut wrong = page;
    // Leaf node header is 11 bytes. An entry starts with a u16 key length,
    // then the u64 physical snapshot page ID. The first key's page ID is at
    // payload byte 13. Change it to 2 and recompute a VALID BRPG checksum.
    wrong.payload[13..21].copy_from_slice(&2_u64.to_le_bytes());
    let reencoded = wrong.encode().unwrap();
    bytes[leaf_offset..leaf_offset + PAGE_SIZE].copy_from_slice(&reencoded);
    fs::write(&path, bytes).unwrap();

    let store = RowStore::open(dir.path()).unwrap();
    // The index page itself has a valid CRC, but the pointer is not a valid
    // representation of the authoritative row checkpoint.
    assert!(store.verify_checkpoint_index().is_err());
    assert_eq!(store.get(b"first"), Some(b"one".to_vec()));
}

#[test]
fn missing_index_recovery_does_not_modify_snapshot_or_wal_bytes() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"one", b"first").unwrap();
        store.checkpoint().unwrap();
        store.put(b"two", b"second").unwrap();
    }
    let snapshot = dir.path().join("snapshot-1.brp");
    let wal = dir.path().join("rows.wal");
    let expected_snapshot = fs::read(&snapshot).unwrap();
    let expected_wal = fs::read(&wal).unwrap();
    fs::remove_file(dir.path().join("index-1.bri")).unwrap();

    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(fs::read(&snapshot).unwrap(), expected_snapshot);
    assert_eq!(fs::read(&wal).unwrap(), expected_wal);
    assert_eq!(store.indexed_get(b"one").unwrap(), Some(b"first".to_vec()));
    assert_eq!(store.indexed_get(b"two").unwrap(), Some(b"second".to_vec()));
    assert_eq!(store.last_lsn(), 2);
}
