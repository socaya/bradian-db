//! G22 durable historical versions in the same authoritative BRWL batch.

use std::sync::{Arc, Barrier};
use std::thread;

use bradian_relational::durable_mvcc::{DurableMvccError, DurableMvccStore};
use bradian_relational::row_store::RowStore;
use tempfile::tempdir;

#[test]
fn historical_versions_survive_checkpoint_and_restart() {
    let dir = tempdir().unwrap();
    let (one, two, three);
    {
        let store = DurableMvccStore::open(dir.path()).unwrap();
        let mut tx = store.begin();
        tx.insert(b"account", b"100").unwrap();
        one = tx.commit().unwrap();
        let mut tx = store.begin();
        tx.replace(b"account", b"200").unwrap();
        two = tx.commit().unwrap();
        let mut tx = store.begin();
        tx.replace(b"account", b"300").unwrap();
        three = tx.commit().unwrap();
        assert_eq!(store.read_as_of(b"account", one).unwrap(), Some(b"100".to_vec()));
        assert_eq!(store.read_as_of(b"account", two).unwrap(), Some(b"200".to_vec()));
        assert_eq!(store.read_as_of(b"account", three).unwrap(), Some(b"300".to_vec()));
        store.checkpoint().unwrap();
    }
    let reopened = DurableMvccStore::open(dir.path()).unwrap();
    assert_eq!(reopened.latest_lsn(), three);
    assert_eq!(reopened.read_as_of(b"account", one).unwrap(), Some(b"100".to_vec()));
    assert_eq!(reopened.read_as_of(b"account", two).unwrap(), Some(b"200".to_vec()));
    assert_eq!(reopened.read_as_of(b"account", three).unwrap(), Some(b"300".to_vec()));
}

#[test]
fn tombstone_and_reinsert_are_historical_and_crash_durable() {
    let dir = tempdir().unwrap();
    let (a, b, c);
    {
        let store = DurableMvccStore::open(dir.path()).unwrap();
        let mut tx = store.begin();
        tx.insert(b"k", b"old").unwrap();
        a = tx.commit().unwrap();
        let mut tx = store.begin();
        tx.delete(b"k").unwrap();
        b = tx.commit().unwrap();
        let mut tx = store.begin();
        tx.insert(b"k", b"new").unwrap();
        c = tx.commit().unwrap();
    }
    let reopened = DurableMvccStore::open(dir.path()).unwrap();
    assert_eq!(reopened.read_as_of(b"k", a).unwrap(), Some(b"old".to_vec()));
    assert_eq!(reopened.read_as_of(b"k", b).unwrap(), None);
    assert_eq!(reopened.read_as_of(b"k", c).unwrap(), Some(b"new".to_vec()));
}

#[test]
fn disjoint_writers_commit_from_same_snapshot() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let mut first = db.begin();
    let mut second = db.begin();
    assert_eq!(first.start_lsn(), second.start_lsn());
    first.insert(b"a", b"1").unwrap();
    second.insert(b"b", b"2").unwrap();
    let a = first.commit().unwrap();
    let b = second.commit().unwrap();
    assert_eq!(b, a + 1);
    assert_eq!(db.read_as_of(b"a", b).unwrap(), Some(b"1".to_vec()));
    assert_eq!(db.read_as_of(b"b", b).unwrap(), Some(b"2".to_vec()));
}

#[test]
fn conflicting_same_key_writes_are_rejected_without_partial_commit() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let mut initial = db.begin();
    initial.insert(b"same", b"original").unwrap();
    initial.commit().unwrap();
    let mut stale = db.begin();
    let mut winner = db.begin();
    stale.replace(b"same", b"stale").unwrap();
    winner.replace(b"same", b"winner").unwrap();
    winner.commit().unwrap();
    let last = db.latest_lsn();
    assert!(matches!(stale.commit(), Err(DurableMvccError::Conflict)));
    assert_eq!(db.latest_lsn(), last);
    assert_eq!(db.read_as_of(b"same", last).unwrap(), Some(b"winner".to_vec()));
}

#[test]
fn full_scan_then_write_conflicts_with_intervening_commit() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let mut old = db.begin();
    assert!(old.scan().is_empty());
    old.insert(b"a", b"1").unwrap();
    let mut other = db.begin();
    other.insert(b"b", b"2").unwrap();
    other.commit().unwrap();
    assert!(matches!(old.commit(), Err(DurableMvccError::ScanConflict)));
    assert_eq!(db.read_as_of(b"a", db.latest_lsn()).unwrap(), None);
}

#[test]
fn rollback_and_dropped_transaction_never_write_history() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let old = db.latest_lsn();
    let mut tx = db.begin();
    tx.insert(b"a", b"1").unwrap();
    tx.rollback();
    {
        let mut tx = db.begin();
        tx.insert(b"b", b"2").unwrap();
    }
    assert_eq!(db.latest_lsn(), old);
    assert_eq!(db.stats().active_snapshots, 0);
    assert_eq!(db.stats().persisted_versions, 0);
}

#[test]
fn retention_floor_is_durable_and_blocks_outdated_as_of_reads() {
    let dir = tempdir().unwrap();
    let (a, b, c);
    {
        let db = DurableMvccStore::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.insert(b"k", b"first").unwrap();
        a = tx.commit().unwrap();
        let mut tx = db.begin();
        tx.replace(b"k", b"second").unwrap();
        b = tx.commit().unwrap();
        let mut tx = db.begin();
        tx.replace(b"k", b"third").unwrap();
        c = tx.commit().unwrap();
        assert_eq!(db.prune_through(b).unwrap(), 1);
        assert!(matches!(
            db.read_as_of(b"k", a), Err(DurableMvccError::SnapshotUnavailable { .. })
        ));
        assert_eq!(db.read_as_of(b"k", b).unwrap(), Some(b"second".to_vec()));
        assert_eq!(db.read_as_of(b"k", c).unwrap(), Some(b"third".to_vec()));
        // Advancing beyond c prunes older baselines.
        assert_eq!(db.prune_through(c).unwrap(), 1);
        db.checkpoint().unwrap();
    }
    let reopened = DurableMvccStore::open(dir.path()).unwrap();
    assert_eq!(reopened.retention_floor(), c);
    assert!(matches!(
        reopened.read_as_of(b"k", b), Err(DurableMvccError::SnapshotUnavailable { .. })
    ));
    assert_eq!(reopened.read_as_of(b"k", c).unwrap(), Some(b"third".to_vec()));
}

#[test]
fn pinned_reader_prevents_advancing_floor_past_its_start() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.insert(b"k", b"one").unwrap();
    tx.commit().unwrap();
    let old = db.begin();
    let mut updater = db.begin();
    updater.replace(b"k", b"two").unwrap();
    let second = updater.commit().unwrap();
    assert!(matches!(db.prune_through(second), Err(DurableMvccError::PinnedSnapshot)));
    assert_eq!(old.get(b"k").unwrap(), Some(b"one".to_vec()));
    old.rollback();
    db.prune_through(second).unwrap();
}

#[test]
fn gc_never_loses_delete_reinsert_baseline_visibility() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = db.begin();
    tx.insert(b"k", b"v1").unwrap();
    tx.commit().unwrap();
    let mut tx = db.begin();
    tx.delete(b"k").unwrap();
    let deleted = tx.commit().unwrap();
    let mut tx = db.begin();
    tx.insert(b"k", b"v2").unwrap();
    let later = tx.commit().unwrap();
    db.prune_through(deleted).unwrap();
    assert_eq!(db.read_as_of(b"k", deleted).unwrap(), None);
    assert_eq!(db.read_as_of(b"k", later).unwrap(), Some(b"v2".to_vec()));
}

#[test]
fn file_format_refuses_existing_plain_g20_or_other_rowstore_data() {
    let dir = tempdir().unwrap();
    {
        let raw = RowStore::open(dir.path()).unwrap();
        raw.put(b"normal-key", b"normal-row").unwrap();
    }
    assert!(matches!(
        DurableMvccStore::open(dir.path()), Err(DurableMvccError::Corruption(_))
    ));
}

#[test]
fn unknown_physical_namespace_in_v1_store_fails_closed() {
    let dir = tempdir().unwrap();
    {
        let db = DurableMvccStore::open(dir.path()).unwrap();
        let mut tx = db.begin();
        tx.insert(b"a", b"value").unwrap();
        tx.commit().unwrap();
    }
    {
        let raw = RowStore::open(dir.path()).unwrap();
        raw.put(b"stray-key", b"stray").unwrap();
    }
    assert!(matches!(DurableMvccStore::open(dir.path()), Err(DurableMvccError::Corruption(_))));
}

#[test]
fn too_many_operations_are_rejected_without_partial_versions() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let before = db.latest_lsn();
    let mut tx = db.begin();
    for index in 0..257 {
        tx.insert(format!("key-{index:03}").as_bytes(), b"value").unwrap();
    }
    assert!(matches!(tx.commit(), Err(DurableMvccError::CapacityExhausted)));
    assert_eq!(db.latest_lsn(), before);
    assert_eq!(db.stats().persisted_versions, 0);
}

#[test]
fn simultaneous_threads_publish_disjoint_durable_histories() {
    let dir = tempdir().unwrap();
    let db = Arc::new(DurableMvccStore::open(dir.path()).unwrap());
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2).map(|i| {
        let db = Arc::clone(&db);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let mut tx = db.begin();
            tx.insert(format!("k{i}").as_bytes(), b"v").unwrap();
            barrier.wait();
            tx.commit()
        })
    }).collect::<Vec<_>>();
    for handle in handles { handle.join().unwrap().unwrap(); }
    assert_eq!(db.stats().persisted_versions, 2);
}

#[test]
fn read_only_transactions_do_not_append_wal_or_change_snapshot() {
    let dir = tempdir().unwrap();
    let db = DurableMvccStore::open(dir.path()).unwrap();
    let old = db.begin();
    let horizon = old.start_lsn();
    let mut writer = db.begin();
    writer.insert(b"k", b"new").unwrap();
    writer.commit().unwrap();
    assert_eq!(old.get(b"k").unwrap(), None);
    assert_eq!(old.commit().unwrap(), horizon);
    assert!(db.latest_lsn() > horizon);
}


#[test]
fn verified_g10_backup_restores_historical_versions_and_floor() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("original");
    let backup = dir.path().join("backup");
    let restored = dir.path().join("restored");
    let (v1, v2, v3);
    {
        let store = DurableMvccStore::open(&root).unwrap();
        let mut tx = store.begin();
        tx.insert(b"account", b"first").unwrap();
        v1 = tx.commit().unwrap();
        let mut tx = store.begin();
        tx.replace(b"account", b"second").unwrap();
        v2 = tx.commit().unwrap();
        let mut tx = store.begin();
        tx.replace(b"account", b"third").unwrap();
        v3 = tx.commit().unwrap();
        store.prune_through(v2).unwrap();
        let report = store.backup_to(&backup).unwrap();
        assert!(report.last_lsn >= v3);
    }
    DurableMvccStore::restore_backup(&backup, &restored).unwrap();
    let store = DurableMvccStore::open(&restored).unwrap();
    assert_eq!(store.retention_floor(), v2);
    assert!(matches!(store.read_as_of(b"account", v1),
        Err(DurableMvccError::SnapshotUnavailable { .. })));
    assert_eq!(store.read_as_of(b"account", v2).unwrap(), Some(b"second".to_vec()));
    assert_eq!(store.read_as_of(b"account", v3).unwrap(), Some(b"third".to_vec()));
}

#[test]
fn retention_floor_advances_even_without_obsolete_version_deletions() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = store.begin();
    tx.insert(b"k", b"value").unwrap();
    let first_commit = tx.commit().unwrap();
    assert_eq!(store.prune_through(first_commit).unwrap(), 0);
    let after_floor = store.latest_lsn();
    assert!(after_floor > first_commit);
    drop(store);
    let restored = DurableMvccStore::open(dir.path()).unwrap();
    assert_eq!(restored.retention_floor(), first_commit);
    assert_eq!(restored.read_as_of(b"k", first_commit).unwrap(), Some(b"value".to_vec()));
}

#[test]
fn key_and_value_limits_fail_before_any_wal_write() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let before = store.latest_lsn();
    let mut tx = store.begin();
    assert!(matches!(tx.insert(&vec![b'k'; 499], b"value"), Err(DurableMvccError::Capacity)));
    assert!(matches!(tx.insert(b"okay", &vec![b'v'; 3546]), Err(DurableMvccError::Capacity)));
    assert_eq!(store.latest_lsn(), before);
    tx.rollback();
}
