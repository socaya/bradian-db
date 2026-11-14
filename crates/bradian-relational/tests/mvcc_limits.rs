//! Bounded in-memory version admission for the opt-in G20 MvccStore (ADR 0058).

use bradian_relational::mvcc::{MvccError, MvccStore};
use bradian_relational::runtime_limits::{MvccStoreLimits, ResourceError};
use tempfile::tempdir;

fn quota(err: MvccError, resource: &str) {
    match err {
        MvccError::Resource(ResourceError::MvccLimit { resource: actual, .. }) => {
            assert_eq!(actual, resource);
        }
        other => panic!("expected MvccLimit({resource}), got {other:?}"),
    }
}

#[test]
fn commit_refuses_additional_distinct_keys_beyond_configured_limit() {
    let dir = tempdir().unwrap();
    let store = MvccStore::open_with_limits(
        dir.path(),
        MvccStoreLimits { max_distinct_keys: 2, max_total_versions: 1024 },
    ).unwrap();
    // Two distinct keys fit the budget.
    {
        let mut tx = store.begin();
        tx.insert(b"a", b"1").unwrap();
        tx.insert(b"b", b"1").unwrap();
        tx.commit().unwrap();
    }
    let committed_before = store.committed_lsn();
    let wal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    // The third distinct key must be refused before any WAL append.
    {
        let mut tx = store.begin();
        tx.insert(b"c", b"1").unwrap();
        quota(tx.commit().unwrap_err(), "distinct keys");
    }
    assert_eq!(store.committed_lsn(), committed_before);
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), wal_before);
    assert_eq!(store.stats().keys_retained, 2);
}

#[test]
fn commit_refuses_additional_versions_beyond_configured_limit() {
    let dir = tempdir().unwrap();
    // One key, two versions is the ceiling. A third version is refused.
    let store = MvccStore::open_with_limits(
        dir.path(),
        MvccStoreLimits { max_distinct_keys: 1024, max_total_versions: 2 },
    ).unwrap();
    // Pin a reader so version chains cannot be pruned down between writes.
    let pinned = store.begin();
    {
        let mut tx = store.begin();
        tx.insert(b"k", b"1").unwrap();
        tx.commit().unwrap();
    }
    {
        let mut tx = store.begin();
        tx.replace(b"k", b"2").unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(store.stats().versions_retained, 2);
    // A third commit would push the chain to 3 while the oldest reader
    // still holds the version from lsn=0, forcing refusal.
    let wal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    {
        let mut tx = store.begin();
        tx.replace(b"k", b"3").unwrap();
        quota(tx.commit().unwrap_err(), "total versions");
    }
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), wal_before);
    drop(pinned);
}

#[test]
fn replacing_an_existing_key_does_not_charge_additional_distinct_capacity() {
    let dir = tempdir().unwrap();
    let store = MvccStore::open_with_limits(
        dir.path(),
        MvccStoreLimits { max_distinct_keys: 1, max_total_versions: 1024 },
    ).unwrap();
    {
        let mut tx = store.begin();
        tx.insert(b"k", b"1").unwrap();
        tx.commit().unwrap();
    }
    // The replace does not grow the set of distinct keys.
    {
        let mut tx = store.begin();
        tx.replace(b"k", b"2").unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(store.stats().keys_retained, 1);
}

#[test]
fn opening_with_tighter_limits_than_committed_state_fails_without_mutating_wal() {
    let dir = tempdir().unwrap();
    {
        let store = MvccStore::open(dir.path()).unwrap();
        for i in 0..4 {
            let mut tx = store.begin();
            tx.insert(format!("k{i}").as_bytes(), b"v").unwrap();
            tx.commit().unwrap();
        }
        store.checkpoint().unwrap();
    }
    let wal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    match MvccStore::open_with_limits(
        dir.path(),
        MvccStoreLimits { max_distinct_keys: 2, max_total_versions: 1024 },
    ) {
        Ok(_) => panic!("tighter reopen must refuse"),
        Err(err) => quota(err, "distinct keys"),
    }
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), wal_before);
    // A relaxed reopen still sees all four keys.
    let relaxed = MvccStore::open(dir.path()).unwrap();
    assert_eq!(relaxed.stats().keys_retained, 4);
}

#[test]
fn default_limits_admit_an_ordinary_mvcc_workload() {
    let dir = tempdir().unwrap();
    let store = MvccStore::open(dir.path()).unwrap();
    for i in 0..64 {
        let mut tx = store.begin();
        tx.insert(format!("k{i:03}").as_bytes(), b"v").unwrap();
        tx.commit().unwrap();
    }
    assert_eq!(store.stats().keys_retained, 64);
    assert_eq!(store.resource_limits().max_distinct_keys, MvccStoreLimits::default().max_distinct_keys);
}
