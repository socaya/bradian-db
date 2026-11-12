//! G20 independent MVCC row kernel: WAL-backed current state with pinned
//! in-memory historical versions and per-key optimistic validation.

use std::sync::{Arc, Barrier};
use std::thread;

use bradian_relational::mvcc::{MvccError, MvccStore};
use tempfile::tempdir;

#[test]
fn committed_snapshot_read_is_stable_across_other_writers() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    {
        let mut initial = store.begin();
        initial.insert(b"a", b"old").unwrap();
        initial.commit().unwrap();
    }
    let reader = store.begin();
    let mut updater = store.begin();
    updater.replace(b"a", b"new").unwrap();
    updater.insert(b"b", b"later").unwrap();
    updater.commit().unwrap();
    assert_eq!(reader.get(b"a"), Some(b"old".to_vec()));
    assert_eq!(reader.get(b"b"), None);
    assert_eq!(store.begin().get(b"a"), Some(b"new".to_vec()));
    assert!(store.stats().versions_retained >= 3);
    drop(reader);
    assert_eq!(store.stats().active_snapshots, 0);
    assert_eq!(store.stats().versions_retained, 2);
}

#[test]
fn disjoint_key_writers_commit_from_same_start_lsn() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut left = store.begin();
    let mut right = store.begin();
    assert_eq!(left.start_lsn(), right.start_lsn());
    left.insert(b"account-a", b"100").unwrap();
    right.insert(b"account-b", b"200").unwrap();
    let first = left.commit().unwrap();
    let second = right.commit().unwrap();
    assert_eq!(second, first + 1);
    assert_eq!(store.begin().get(b"account-a"), Some(b"100".to_vec()));
    assert_eq!(store.begin().get(b"account-b"), Some(b"200".to_vec()));
}

#[test]
fn same_key_concurrent_update_refuses_lost_update() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut initial = store.begin();
    initial.insert(b"balance", b"1000").unwrap();
    initial.commit().unwrap();
    let mut first = store.begin();
    let mut stale = store.begin();
    first.replace(b"balance", b"950").unwrap();
    stale.replace(b"balance", b"900").unwrap();
    first.commit().unwrap();
    assert!(matches!(stale.commit(), Err(MvccError::WriteConflict)));
    assert_eq!(store.begin().get(b"balance"), Some(b"950".to_vec()));
}

#[test]
fn delete_reinsert_aba_is_detected_as_newer_key_version() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut initial = store.begin();
    initial.insert(b"k", b"original").unwrap();
    initial.commit().unwrap();
    let mut stale = store.begin();
    stale.replace(b"k", b"stale-overwrite").unwrap();
    let mut deleting = store.begin();
    deleting.delete(b"k").unwrap();
    deleting.commit().unwrap();
    let mut reinserting = store.begin();
    reinserting.insert(b"k", b"original").unwrap();
    reinserting.commit().unwrap();
    assert!(matches!(stale.commit(), Err(MvccError::WriteConflict)));
    assert_eq!(store.begin().get(b"k"), Some(b"original".to_vec()));
}

#[test]
fn scan_protects_against_new_rows_and_phantoms() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut reader_writer = store.begin();
    assert!(reader_writer.scan().is_empty());
    reader_writer.insert(b"a", b"1").unwrap();
    let mut concurrent = store.begin();
    concurrent.insert(b"phantom", b"visible-only-to-future").unwrap();
    concurrent.commit().unwrap();
    assert!(matches!(reader_writer.commit(), Err(MvccError::ScanConflict)));
    assert!(store.begin().get(b"a").is_none());
    assert_eq!(store.begin().get(b"phantom"), Some(b"visible-only-to-future".to_vec()));
}

#[test]
fn scan_tracks_read_your_writes_and_deletes() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut initial = store.begin();
    initial.insert(b"a", b"old").unwrap();
    initial.insert(b"b", b"old").unwrap();
    initial.commit().unwrap();
    let mut tx = store.begin();
    tx.replace(b"a", b"new").unwrap();
    tx.delete(b"b").unwrap();
    tx.insert(b"c", b"new").unwrap();
    assert_eq!(tx.scan(), vec![
        (b"a".to_vec(), b"new".to_vec()),
        (b"c".to_vec(), b"new".to_vec()),
    ]);
    tx.rollback();
    let mut check = store.begin();
    assert_eq!(check.scan(), vec![
        (b"a".to_vec(), b"old".to_vec()),
        (b"b".to_vec(), b"old".to_vec()),
    ]);
}

#[test]
fn rollback_and_implicit_drop_do_not_advance_wal() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let lsn = store.committed_lsn();
    {
        let mut tx = store.begin();
        tx.insert(b"a", b"value").unwrap();
        tx.rollback();
    }
    {
        let mut tx = store.begin();
        tx.insert(b"b", b"uncommitted").unwrap();
    }
    assert_eq!(store.committed_lsn(), lsn);
    assert_eq!(store.stats().active_snapshots, 0);
    assert_eq!(store.begin().get(b"a"), None);
    assert_eq!(store.begin().get(b"b"), None);
}

#[test]
fn checkpoint_and_restart_recover_latest_values_not_old_versions() {
    let root = tempdir().unwrap();
    let last_lsn;
    {
        let store = MvccStore::open(root.path()).unwrap();
        let mut tx = store.begin();
        tx.insert(b"k", b"version-one").unwrap();
        tx.commit().unwrap();
        let old = store.begin();
        let mut tx = store.begin();
        tx.replace(b"k", b"version-two").unwrap();
        last_lsn = tx.commit().unwrap();
        assert_eq!(old.get(b"k"), Some(b"version-one".to_vec()));
        store.checkpoint().unwrap();
        assert_eq!(old.get(b"k"), Some(b"version-one".to_vec()));
    }
    let reopened = MvccStore::open(root.path()).unwrap();
    assert_eq!(reopened.committed_lsn(), last_lsn);
    assert_eq!(reopened.begin().get(b"k"), Some(b"version-two".to_vec()));
    assert_eq!(reopened.stats().versions_retained, 1);
}

#[test]
fn multiple_readers_at_same_lsn_are_independently_pinned() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let first = store.begin();
    let second = store.begin();
    assert_eq!(store.stats().active_snapshots, 2);
    assert_eq!(store.stats().pinned_positions, 1);
    drop(first);
    assert_eq!(store.stats().active_snapshots, 1);
    drop(second);
    assert_eq!(store.stats().active_snapshots, 0);
}

#[test]
fn old_versions_are_reclaimed_only_after_oldest_reader_exits() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut initial = store.begin();
    initial.insert(b"x", b"v0").unwrap();
    initial.commit().unwrap();
    let oldest = store.begin();
    for i in 1..=5 {
        let mut writer = store.begin();
        writer.replace(b"x", format!("v{i}").as_bytes()).unwrap();
        writer.commit().unwrap();
    }
    assert_eq!(oldest.get(b"x"), Some(b"v0".to_vec()));
    assert!(store.stats().versions_retained >= 6);
    drop(oldest);
    assert_eq!(store.stats().versions_retained, 1);
}

#[test]
fn duplicate_key_and_missing_key_are_rejected_without_wal_write() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let mut tx = store.begin();
    tx.insert(b"a", b"first").unwrap();
    assert!(matches!(tx.insert(b"a", b"duplicate"), Err(MvccError::DuplicateKey)));
    assert!(matches!(tx.replace(b"missing", b"value"), Err(MvccError::NotFound)));
    assert!(matches!(tx.delete(b"missing"), Err(MvccError::NotFound)));
    tx.commit().unwrap();
    assert_eq!(store.begin().get(b"a"), Some(b"first".to_vec()));
}

#[test]
fn wal_batch_limit_rejects_whole_mvcc_commit_without_partial_visibility() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let before = store.committed_lsn();
    let mut tx = store.begin();
    for i in 0..257 {
        tx.insert(format!("item-{i:03}").as_bytes(), b"v").unwrap();
    }
    assert!(matches!(tx.commit(), Err(MvccError::Storage(_))));
    assert_eq!(store.committed_lsn(), before);
    assert_eq!(store.stats().active_snapshots, 0);
    assert!(store.begin().scan().is_empty());
}

#[test]
fn concurrent_disjoint_writers_publish_both_without_conflict() {
    let root = tempdir().unwrap();
    let store = Arc::new(MvccStore::open(root.path()).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2).map(|i| {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut tx = store.begin();
            tx.insert(format!("key-{i}").as_bytes(), b"committed").unwrap();
            barrier.wait();
            tx.commit()
        })
    }).collect::<Vec<_>>();
    for handle in handles { handle.join().unwrap().unwrap(); }
    assert_eq!(store.begin().scan().len(), 2);
}

#[test]
fn read_only_snapshot_never_writes_or_claims_latest_lsn() {
    let root = tempdir().unwrap();
    let store = MvccStore::open(root.path()).unwrap();
    let old = store.begin();
    let start = old.start_lsn();
    let mut writer = store.begin();
    writer.insert(b"new", b"row").unwrap();
    writer.commit().unwrap();
    assert_eq!(old.commit().unwrap(), start);
    assert!(store.committed_lsn() > start);
}
