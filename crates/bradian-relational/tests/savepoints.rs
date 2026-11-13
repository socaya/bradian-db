//! G41 native durable savepoint semantics and conflict preservation.
use bradian_relational::durable_mvcc::{DurableMvccError, DurableMvccStore};
use tempfile::tempdir;

#[test]
fn shadowed_markers_restore_inserts_updates_and_deletes_without_wal_writes() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = store.begin();
    tx.insert(b"a", b"old").unwrap();
    tx.insert(b"b", b"old").unwrap();
    let baseline = tx.commit().unwrap();
    let mut tx = store.begin();
    tx.replace(b"a", b"kept").unwrap();
    tx.savepoint("same").unwrap();
    tx.delete(b"b").unwrap();
    tx.savepoint("same").unwrap();
    tx.insert(b"c", b"discarded").unwrap();
    tx.rollback_to("same").unwrap();
    assert_eq!(tx.get(b"c").unwrap(), None);
    assert_eq!(tx.get(b"b").unwrap(), None);
    tx.release_savepoint("same").unwrap();
    tx.rollback_to("same").unwrap();
    assert_eq!(tx.get(b"b").unwrap(), Some(b"old".to_vec()));
    tx.savepoint("child").unwrap();
    tx.insert(b"d", b"kept").unwrap();
    tx.release_savepoint("same").unwrap();
    assert_eq!(tx.savepoint_count(), 0);
    assert!(tx.rollback_to("child").is_err());
    assert_eq!(store.latest_lsn(), baseline);
    assert_eq!(store.stats().active_snapshots, 1);
    assert_eq!(tx.start_lsn(), baseline);
    assert_eq!(tx.commit().unwrap(), baseline + 1);
    assert_eq!(store.stats().active_snapshots, 0);
    drop(store);
    let store = DurableMvccStore::open(dir.path()).unwrap();
    assert_eq!(store.read_as_of(b"a", baseline + 1).unwrap(), Some(b"kept".to_vec()));
    assert_eq!(store.read_as_of(b"d", baseline + 1).unwrap(), Some(b"kept".to_vec()));
    assert_eq!(store.read_as_of(b"c", baseline + 1).unwrap(), None);
}

#[test]
fn invalid_names_marker_limits_and_staged_key_limits_do_not_mutate_state() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let baseline = store.latest_lsn();
    let mut tx = store.begin();
    for name in ["", "1bad", "bad-name", "é", &"x".repeat(64)] {
        assert!(matches!(tx.savepoint(name), Err(DurableMvccError::Savepoint(_))));
        assert_eq!(tx.savepoint_count(), 0);
    }
    tx.savepoint(&"x".repeat(63)).unwrap();
    tx.release_savepoint(&"x".repeat(63)).unwrap();
    for i in 0..16 { tx.savepoint(&format!("s{i}")).unwrap(); }
    assert!(tx.savepoint("overflow").is_err());
    assert_eq!(tx.savepoint_count(), 16);
    tx.rollback_to("s0").unwrap();
    for i in 0_u32..257 { tx.insert(&i.to_be_bytes(), b"v").unwrap(); }
    assert!(tx.savepoint("too_many_keys").is_err());
    assert_eq!(tx.savepoint_count(), 1);
    assert!(tx.rollback_to("absent").is_err());
    assert!(tx.release_savepoint("absent").is_err());
    assert_eq!(tx.get(&256_u32.to_be_bytes()).unwrap(), Some(b"v".to_vec()));
    tx.rollback_to("s0").unwrap();
    tx.release_savepoint("s0").unwrap();
    assert_eq!(tx.commit().unwrap(), baseline);
    assert_eq!(store.stats().active_snapshots, 0);
    assert_eq!(store.latest_lsn(), baseline);
}

#[test]
fn rollback_to_never_forgets_full_scan_conflict_observation() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = store.begin();
    tx.savepoint("before_scan").unwrap();
    tx.scan();
    let mut other = store.begin();
    other.insert(b"other", b"v").unwrap();
    let lsn = other.commit().unwrap();
    tx.rollback_to("before_scan").unwrap();
    tx.insert(b"independent", b"v").unwrap();
    assert!(matches!(tx.commit(), Err(DurableMvccError::ScanConflict)));
    assert_eq!(store.latest_lsn(), lsn);
    assert_eq!(store.stats().active_snapshots, 0);
}

#[test]
fn restored_writes_keep_original_snapshot_conflicts_and_retention_pin() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut seed = store.begin();
    seed.insert(b"k", b"old").unwrap();
    seed.commit().unwrap();
    let mut tx = store.begin();
    tx.replace(b"k", b"mine").unwrap();
    tx.savepoint("written").unwrap();
    let mut other = store.begin();
    other.replace(b"k", b"winner").unwrap();
    let winner = other.commit().unwrap();
    assert!(matches!(store.prune_through(winner), Err(DurableMvccError::PinnedSnapshot)));
    tx.rollback_to("written").unwrap();
    tx.release_savepoint("written").unwrap();
    assert!(matches!(tx.commit(), Err(DurableMvccError::Conflict)));
    assert_eq!(store.stats().active_snapshots, 0);
    assert_eq!(store.read_as_of(b"k", winner).unwrap(), Some(b"winner".to_vec()));
    store.prune_through(winner).unwrap();
}

#[test]
fn dropping_or_rolling_back_marked_transactions_releases_the_single_pin() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    let baseline = store.latest_lsn();
    for rollback in [false, true] {
        let mut tx = store.begin();
        tx.savepoint("a").unwrap();
        tx.insert(b"discard", b"v").unwrap();
        tx.savepoint("b").unwrap();
        if rollback { tx.rollback(); } else { drop(tx); }
        assert_eq!(store.stats().active_snapshots, 0);
        assert_eq!(store.latest_lsn(), baseline);
    }
}
