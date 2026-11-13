//! G42: read observation validation under the atomic commit mutex.
use bradian_relational::durable_mvcc::{DurableMvccError, DurableMvccStore};
use std::sync::{Arc, Barrier};
use tempfile::tempdir;

fn seed(store: &DurableMvccStore) {
    let mut tx = store.begin();
    tx.insert(b"a", b"1").unwrap();
    tx.insert(b"b", b"1").unwrap();
    tx.commit().unwrap();
}

#[test]
fn concurrent_validating_writers_cannot_both_commit_write_skew() {
    for validate in [false, true] {
        let dir = tempdir().unwrap();
        let store = Arc::new(DurableMvccStore::open(dir.path()).unwrap());
        seed(&store);
        let before = store.latest_lsn();
        let barrier = Arc::new(Barrier::new(2));
        let mut threads = Vec::new();
        for (own, other) in [(b"a", b"b"), (b"b", b"a")] {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            threads.push(std::thread::spawn(move || {
                let mut tx = if validate { store.begin_with_read_validation() } else { store.begin() };
                assert_eq!(tx.read_validation_enabled(), validate);
                assert_eq!(tx.get(other).unwrap(), Some(b"1".to_vec()));
                tx.replace(own, b"0").unwrap();
                barrier.wait();
                tx.commit()
            }));
        }
        let results = threads.into_iter().map(|t| t.join().unwrap()).collect::<Vec<_>>();
        let successes = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(successes, if validate { 1 } else { 2 });
        for error in results.into_iter().filter_map(Result::err) {
            assert!(matches!(error, DurableMvccError::ReadConflict));
        }
        assert_eq!(store.latest_lsn(), before + successes as u64);
        assert_eq!(store.stats().active_snapshots, 0);
        let a = store.read_as_of(b"a", store.latest_lsn()).unwrap().unwrap();
        let b = store.read_as_of(b"b", store.latest_lsn()).unwrap().unwrap();
        assert_eq!(a == b"1" || b == b"1", validate);
    }
}

#[test]
fn independent_validated_writes_commit_and_read_only_validation_does_not_write_wal() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    seed(&store);
    let mut first = store.begin_with_read_validation();
    let mut second = store.begin_with_read_validation();
    first.replace(b"a", b"2").unwrap();
    second.replace(b"b", b"2").unwrap();
    first.commit().unwrap();
    second.commit().unwrap();
    let reader = store.begin_with_read_validation();
    assert_eq!(reader.get(b"a").unwrap(), Some(b"2".to_vec()));
    let snapshot = reader.start_lsn();
    let mut writer = store.begin();
    writer.insert(b"unrelated", b"v").unwrap();
    let latest = writer.commit().unwrap();
    assert_eq!(reader.commit().unwrap(), snapshot);
    assert_eq!(store.latest_lsn(), latest);
    let stale = store.begin_with_read_validation();
    stale.get(b"a").unwrap();
    let mut writer = store.begin();
    writer.replace(b"a", b"3").unwrap();
    let latest = writer.commit().unwrap();
    assert!(matches!(stale.commit(), Err(DurableMvccError::ReadConflict)));
    assert_eq!(store.latest_lsn(), latest);
    assert_eq!(store.stats().active_snapshots, 0);
}

#[test]
fn absent_keys_tombstones_and_aba_values_are_conflicts() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    seed(&store);
    for defect in ["absent", "delete", "aba"] {
        let reader = store.begin_with_read_validation();
        let key: &[u8] = if defect == "absent" { b"missing" } else { b"a" };
        let observed = reader.get(key).unwrap();
        let mut writer = store.begin();
        if defect == "absent" { writer.insert(key, b"new").unwrap(); }
        else if defect == "delete" { writer.delete(key).unwrap(); }
        else { writer.replace(key, b"different").unwrap(); }
        writer.commit().unwrap();
        if defect != "absent" {
            let mut writer = store.begin();
            if defect == "delete" { writer.insert(key, &observed.clone().unwrap()).unwrap(); }
            else { writer.replace(key, &observed.clone().unwrap()).unwrap(); }
            writer.commit().unwrap();
        }
        assert!(matches!(reader.commit(), Err(DurableMvccError::ReadConflict)));
        assert_eq!(store.stats().active_snapshots, 0);
    }
}

#[test]
fn savepoint_rollback_and_release_cannot_forget_observations() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    seed(&store);
    for release in [false, true] {
        let mut tx = store.begin_with_read_validation();
        tx.savepoint("before_read").unwrap();
        tx.get(b"a").unwrap();
        if release { tx.release_savepoint("before_read").unwrap(); }
        else { tx.rollback_to("before_read").unwrap(); }
        tx.replace(b"b", b"0").unwrap();
        let mut writer = store.begin();
        writer.replace(b"a", b"changed").unwrap();
        let latest = writer.commit().unwrap();
        assert!(matches!(tx.commit(), Err(DurableMvccError::ReadConflict)));
        assert_eq!(store.latest_lsn(), latest);
        assert_eq!(store.read_as_of(b"b", latest).unwrap(), Some(b"1".to_vec()));
    }
}

#[test]
fn scans_including_empty_results_validate_read_only_and_writing_commits() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    for write in [false, true] {
        let mut reader = store.begin_with_read_validation();
        reader.scan();
        reader.savepoint("after_scan").unwrap();
        let mut writer = store.begin();
        let key = if write { b"other2" } else { b"other1" };
        writer.insert(key, b"v").unwrap();
        let latest = writer.commit().unwrap();
        reader.rollback_to("after_scan").unwrap();
        if write { reader.insert(b"mine", b"v").unwrap(); }
        assert!(matches!(reader.commit(), Err(DurableMvccError::ScanConflict)));
        assert_eq!(store.latest_lsn(), latest);
        assert_eq!(store.stats().active_snapshots, 0);
    }
}

#[test]
fn observation_limit_falls_back_to_bounded_domain_validation() {
    let dir = tempdir().unwrap();
    let store = DurableMvccStore::open(dir.path()).unwrap();
    for count in [4096_u32, 4097] {
        let mut reader = store.begin_with_read_validation();
        reader.savepoint("start").unwrap();
        for i in 0..count { assert_eq!(reader.get(&i.to_be_bytes()).unwrap(), None); }
        // Repeated keys must not consume more tracking capacity.
        for _ in 0..8 { reader.get(&0_u32.to_be_bytes()).unwrap(); }
        reader.rollback_to("start").unwrap();
        let mut writer = store.begin();
        writer.insert(if count == 4096 { b"unrelated1" } else { b"unrelated2" }, b"v").unwrap();
        let latest = writer.commit().unwrap();
        let result = reader.commit();
        if count == 4096 { assert!(result.is_ok()); }
        else { assert!(matches!(result, Err(DurableMvccError::ScanConflict))); }
        assert_eq!(store.latest_lsn(), latest);
    }
}

#[test]
fn all_twenty_interleavings_of_two_transactions_preserve_shared_rule() {
    // Enumerate all C(6,3) interleavings, preserving read/write/commit order
    // within each transaction. Both begin before any scheduled step.
    for first in 0..6 {
        for second in first + 1..6 {
            for third in second + 1..6 {
                let dir = tempdir().unwrap();
                let store = DurableMvccStore::open(dir.path()).unwrap();
                seed(&store);
                let mut transactions = [Some(store.begin_with_read_validation()), Some(store.begin_with_read_validation())];
                let mut step = [0; 2];
                let mut enabled = [false; 2];
                let mut successes = 0;
                for position in 0..6 {
                    let t = usize::from(![first, second, third].contains(&position));
                    let own: &[u8] = if t == 0 { b"a" } else { b"b" };
                    let other: &[u8] = if t == 0 { b"b" } else { b"a" };
                    match step[t] {
                        0 => enabled[t] = transactions[t].as_ref().unwrap().get(other).unwrap() == Some(b"1".to_vec()),
                        1 => if enabled[t] { transactions[t].as_mut().unwrap().replace(own, b"0").unwrap(); },
                        2 => match transactions[t].take().unwrap().commit() {
                            Ok(_) => successes += 1,
                            Err(error) => assert!(matches!(error, DurableMvccError::ReadConflict | DurableMvccError::Conflict)),
                        },
                        _ => unreachable!(),
                    }
                    step[t] += 1;
                }
                assert!(successes >= 1);
                let latest = store.latest_lsn();
                assert!(store.read_as_of(b"a", latest).unwrap() == Some(b"1".to_vec())
                    || store.read_as_of(b"b", latest).unwrap() == Some(b"1".to_vec()));
                assert_eq!(store.stats().active_snapshots, 0);
            }
        }
    }
}
