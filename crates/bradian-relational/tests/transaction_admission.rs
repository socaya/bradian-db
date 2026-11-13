use std::sync::{Arc,Barrier};
use bradian_relational::durable_mvcc::{DurableMvccStore,DurableMvccError};
use bradian_relational::runtime_limits::{HistoryLimits,TransactionLimits,ResourceError};
fn open(path:&std::path::Path,max_active:usize)->DurableMvccStore {
    DurableMvccStore::open_with_resource_limits(path,HistoryLimits::default(),TransactionLimits { max_active }).unwrap()
}
fn quota<T>(result:Result<T,DurableMvccError>) { assert!(matches!(result,Err(DurableMvccError::Resource(ResourceError::TransactionLimit { .. })))); }

#[test]
fn active_handles_at_same_horizon_are_counted_before_any_new_pin_or_wal_change() {
    let dir=tempfile::tempdir().unwrap(); let db=open(dir.path(),2); let lsn=db.latest_lsn();
    let a=db.try_begin().unwrap(); let b=db.try_begin_with_read_validation().unwrap(); assert_eq!(a.start_lsn(),b.start_lsn());
    let wal=std::fs::read(dir.path().join("rows.wal")).unwrap(); quota(db.try_begin());
    assert_eq!(db.stats().active_snapshots,2); assert_eq!(db.latest_lsn(),lsn); assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(),wal);
    a.rollback(); assert_eq!(db.stats().active_snapshots,1); let c=db.try_begin().unwrap(); drop(b); drop(c); assert_eq!(db.stats().active_snapshots,0);
}

#[test]
fn distinct_horizons_remain_pinned_until_their_admitted_handles_release() {
    let dir=tempfile::tempdir().unwrap(); let db=open(dir.path(),2);
    let mut seed=db.try_begin().unwrap(); seed.insert(b"a",b"old").unwrap(); seed.commit().unwrap();
    let old=db.try_begin().unwrap(); let mut writer=db.try_begin().unwrap(); writer.replace(b"a",b"new").unwrap(); let floor=writer.commit().unwrap(); let current=db.try_begin().unwrap();
    assert_ne!(old.start_lsn(),current.start_lsn()); quota(db.try_begin());
    assert!(matches!(db.prune_through(floor),Err(DurableMvccError::PinnedSnapshot)));
    drop(old); assert_eq!(db.prune_through(floor).unwrap(),1); assert_eq!(current.get(b"a").unwrap(),Some(b"new".to_vec()));
    drop(current); assert_eq!(db.stats().active_snapshots,0);
}

#[test]
fn successful_read_only_validating_write_rollback_and_drop_paths_refund_slots() {
    let dir=tempfile::tempdir().unwrap(); let db=open(dir.path(),1);
    db.try_begin().unwrap().commit().unwrap(); assert_eq!(db.stats().active_snapshots,0);
    db.try_begin_with_read_validation().unwrap().commit().unwrap(); assert_eq!(db.stats().active_snapshots,0);
    let mut tx=db.try_begin().unwrap(); tx.insert(b"a",b"v").unwrap(); tx.commit().unwrap(); assert_eq!(db.stats().active_snapshots,0);
    db.try_begin().unwrap().rollback(); assert_eq!(db.stats().active_snapshots,0);
    drop(db.try_begin().unwrap()); assert_eq!(db.stats().active_snapshots,0);
}

#[test]
fn commit_conflict_and_retained_history_refusal_release_admitted_capacity() {
    let dir=tempfile::tempdir().unwrap(); let db=open(dir.path(),2); let mut a=db.try_begin().unwrap(); let mut b=db.try_begin().unwrap();
    a.insert(b"a",b"one").unwrap(); b.insert(b"a",b"two").unwrap(); a.commit().unwrap(); assert!(matches!(b.commit(),Err(DurableMvccError::Conflict))); assert_eq!(db.stats().active_snapshots,0); drop(db);
    let other=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open_with_resource_limits(other.path(),HistoryLimits { max_versions:0,..HistoryLimits::default() },TransactionLimits { max_active:1 }).unwrap();
    let mut tx=db.try_begin().unwrap(); tx.insert(b"a",b"v").unwrap(); assert!(matches!(tx.commit(),Err(DurableMvccError::Resource(ResourceError::HistoryLimit { .. }))));
    assert_eq!(db.stats().active_snapshots,0); drop(db.try_begin().unwrap());
}

#[test]
fn simultaneous_admission_cannot_overbook_shared_capacity() {
    let dir=tempfile::tempdir().unwrap(); let db=Arc::new(open(dir.path(),4)); let start=Arc::new(Barrier::new(16)); let held=Arc::new(Barrier::new(16));
    let threads=(0..16).map(|_|{ let db=Arc::clone(&db); let start=Arc::clone(&start); let held=Arc::clone(&held);
        std::thread::spawn(move||{ start.wait(); let tx=db.try_begin(); let admitted=tx.is_ok(); held.wait(); drop(tx); admitted })
    }).collect::<Vec<_>>();
    assert_eq!(threads.into_iter().map(|h|h.join().unwrap()).filter(|ok|*ok).count(),4);
    assert_eq!(db.stats().active_snapshots,0); db.try_begin().unwrap().commit().unwrap();
}

#[test]
fn compatibility_pins_are_counted_and_policy_is_per_owner_not_persisted_or_global() {
    let dir=tempfile::tempdir().unwrap(); let a_path=dir.path().join("a"); let db=open(&a_path,1); let administrative=db.begin(); quota(db.try_begin()); drop(administrative);
    let a=db.try_begin().unwrap(); let other=open(&dir.path().join("b"),1); let b=other.try_begin().unwrap(); drop(a); drop(b); drop(db);
    let db=DurableMvccStore::open(&a_path).unwrap(); assert_eq!(db.transaction_limits().max_active,128);
    let zero=open(&dir.path().join("zero"),0); quota(zero.try_begin()); assert_eq!(zero.stats().active_snapshots,0);
    // Infallible administrative compatibility APIs retain their original contract.
    let admin=zero.begin(); assert_eq!(zero.stats().active_snapshots,1); drop(admin);
}
