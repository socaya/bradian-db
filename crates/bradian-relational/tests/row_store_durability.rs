//! End-to-end native row storage, WAL replay, checkpoint and corruption coverage.
//!
//! Tests intentionally exercise actual files, restarts and torn WAL tails.
//! This is not a test of multi-process exclusivity or power-loss durability.

use std::fs::{self, OpenOptions};
use std::io::Write;

use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

fn open(path: &std::path::Path) -> RowStore {
    RowStore::open(path).unwrap()
}

#[test]
fn committed_upserts_and_deletes_survive_reopen_without_checkpoint() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        assert_eq!(store.put(b"alice", b"first").unwrap(), 1);
        assert_eq!(store.put(b"bob", b"second").unwrap(), 2);
        assert_eq!(store.put(b"alice", b"updated").unwrap(), 3);
        assert_eq!(store.delete(b"bob").unwrap(), 4);
        assert_eq!(store.len(), 1);
        assert_eq!(store.get(b"alice"), Some(b"updated".to_vec()));
    }
    let store = open(dir.path());
    assert_eq!(store.last_lsn(), 4);
    assert_eq!(store.get(b"alice"), Some(b"updated".to_vec()));
    assert_eq!(store.get(b"bob"), None);
    assert_eq!(store.scan(), vec![(b"alice".to_vec(), b"updated".to_vec())]);
    assert_eq!(store.put(b"charlie", b"third").unwrap(), 5);
    drop(store);
    assert_eq!(open(dir.path()).get(b"charlie"), Some(b"third".to_vec()));
}

#[test]
fn checkpoint_publishes_multiple_physical_pages_and_clears_wal() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        for i in 0..120 {
            let key = format!("key-{i:04}");
            let value = vec![i as u8; 100];
            store.put(key.as_bytes(), &value).unwrap();
        }
        assert_eq!(store.checkpoint().unwrap(), 120);
        assert_eq!(fs::metadata(dir.path().join("rows.wal")).unwrap().len(), 0);
        let path = dir.path().join("snapshot-120.brp");
        assert!(fs::metadata(path).unwrap().len() > 4096);
        assert_eq!(store.checkpoint().unwrap(), 120, "idempotent checkpoint");
        store.put(b"key-later", b"after-checkpoint").unwrap();
    }
    let store = open(dir.path());
    assert_eq!(store.len(), 121);
    assert_eq!(store.last_lsn(), 121);
    assert_eq!(store.get(b"key-0002"), Some(vec![2; 100]));
    assert_eq!(store.get(b"key-later"), Some(b"after-checkpoint".to_vec()));
}

#[test]
fn restart_repairs_truncated_wal_tail_before_new_appends() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"a", b"one").unwrap();
        store.put(b"b", b"two").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    {
        let mut file = OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(b"BRWL\x1e").unwrap(); // partial header, never committed
        file.sync_all().unwrap();
    }
    let store = open(dir.path());
    assert_eq!(store.last_lsn(), 2);
    assert_eq!(store.get(b"b"), Some(b"two".to_vec()));
    store.put(b"c", b"three").unwrap();
    drop(store);
    let restarted = open(dir.path());
    assert_eq!(restarted.last_lsn(), 3);
    assert_eq!(restarted.get(b"c"), Some(b"three".to_vec()));
}

#[test]
fn restart_repairs_truncated_record_body() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"first", b"durable").unwrap();
    }
    let wal = dir.path().join("rows.wal");
    {
        let mut file = OpenOptions::new().append(true).open(wal).unwrap();
        // A complete header but only three bytes of the declared body.
        file.write_all(b"BRWL").unwrap();
        file.write_all(&(20_u32).to_le_bytes()).unwrap();
        file.write_all(&(0_u32).to_le_bytes()).unwrap();
        file.write_all(b"abc").unwrap();
        file.sync_all().unwrap();
    }
    let store = open(dir.path());
    assert_eq!(store.get(b"first"), Some(b"durable".to_vec()));
    assert_eq!(store.put(b"second", b"recovered").unwrap(), 2);
}

#[test]
fn wal_crc_corruption_must_fail_closed() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"one", b"first").unwrap();
        store.put(b"two", b"second").unwrap();
    }
    let path = dir.path().join("rows.wal");
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xaa;
    fs::write(&path, bytes).unwrap();
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Corruption(_))));
}

#[test]
fn snapshot_corruption_cannot_silently_fall_back_to_old_data() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"id", b"previous").unwrap();
        store.checkpoint().unwrap();
        store.put(b"id", b"current").unwrap();
        store.checkpoint().unwrap();
    }
    let current = dir.path().join("snapshot-2.brp");
    let mut bytes = fs::read(&current).unwrap();
    bytes[40] ^= 0x55;
    fs::write(current, bytes).unwrap();
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Page(_))));
}

#[test]
fn torn_unpublished_snapshot_does_not_replace_committed_wal_state() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"safe", b"committed").unwrap();
        // An interrupted checkpoint uses .tmp; only .brp files are published.
    }
    fs::write(dir.path().join("snapshot-1.tmp"), b"partial").unwrap();
    assert_eq!(open(dir.path()).get(b"safe"), Some(b"committed".to_vec()));
}

#[test]
fn latest_checkpoint_remains_authoritative_when_wal_was_not_truncated() {
    let dir = tempdir().unwrap();
    let wal_before;
    {
        let store = open(dir.path());
        store.put(b"k", b"first").unwrap();
        store.put(b"k", b"second").unwrap();
        wal_before = fs::read(dir.path().join("rows.wal")).unwrap();
        store.checkpoint().unwrap();
    }
    // Simulate a crash after snapshot publication but before WAL truncation.
    fs::write(dir.path().join("rows.wal"), wal_before).unwrap();
    let store = open(dir.path());
    assert_eq!(store.last_lsn(), 2);
    assert_eq!(store.get(b"k"), Some(b"second".to_vec()));
    assert_eq!(store.put(b"next", b"value").unwrap(), 3);
}

#[test]
fn rejects_invalid_rows_before_writing_wal() {
    let dir = tempdir().unwrap();
    let store = open(dir.path());
    assert!(matches!(store.put(b"", b"value"), Err(StoreError::InvalidRow(_))));
    assert!(matches!(store.put(&vec![b'a'; 513], b"value"), Err(StoreError::InvalidRow(_))));
    assert!(matches!(store.put(b"id", &vec![0; 3550]), Err(StoreError::InvalidRow(_))));
    assert!(matches!(store.delete(b""), Err(StoreError::InvalidRow(_))));
    assert!(store.is_empty());
    assert_eq!(store.last_lsn(), 0);
    assert_eq!(fs::metadata(dir.path().join("rows.wal")).unwrap().len(), 0);
}

#[test]
fn empty_store_checkpoint_and_ordered_binary_keys() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        assert_eq!(store.checkpoint().unwrap(), 0);
        store.put(b"\xff", b"z").unwrap();
        store.put(b"\x00", b"a").unwrap();
    }
    let store = open(dir.path());
    assert_eq!(store.scan(), vec![
        (b"\x00".to_vec(), b"a".to_vec()),
        (b"\xff".to_vec(), b"z".to_vec()),
    ]);
}

#[test]
fn repeated_checkpoint_keeps_newest_visible_state() {
    let dir = tempdir().unwrap();
    {
        let store = open(dir.path());
        store.put(b"x", b"old").unwrap();
        store.checkpoint().unwrap();
        store.delete(b"x").unwrap();
        store.checkpoint().unwrap();
        store.put(b"x", b"new").unwrap();
        store.checkpoint().unwrap();
    }
    let store = open(dir.path());
    assert_eq!(store.last_lsn(), 3);
    assert_eq!(store.get(b"x"), Some(b"new".to_vec()));
    assert_eq!(store.len(), 1);
}
