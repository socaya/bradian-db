use bradian_relational::durable_mvcc::{DurableMvccError,DurableMvccStore};
use bradian_relational::runtime_limits::{HistoryLimits,HistoryUsage,ResourceError};
use std::sync::{Arc,Barrier};

fn limits(keys:usize,versions:usize,bytes:usize)->HistoryLimits { HistoryLimits { max_keys:keys,max_versions:versions,max_bytes:bytes } }
fn quota(error:DurableMvccError,expected:&str) {
    assert!(matches!(error,DurableMvccError::Resource(ResourceError::HistoryLimit { resource,.. }) if resource==expected));
}

#[test]
fn whole_batch_is_rejected_before_wal_or_usage_changes_at_each_limit() {
    let probe=tempfile::tempdir().unwrap(); let p=DurableMvccStore::open(probe.path()).unwrap();
    let mut tx=p.begin(); tx.insert(b"a",b"old").unwrap(); tx.commit().unwrap();
    let mut tx=p.begin(); tx.replace(b"a",b"new").unwrap(); tx.insert(b"b",b"new").unwrap(); tx.commit().unwrap();
    let needed=p.history_usage().bytes;
    for (policy,resource) in [(limits(1,3,10000),"keys"),(limits(2,2,10000),"versions"),(limits(2,3,needed-1),"bytes")] {
        let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open_with_limits(dir.path(),policy).unwrap();
        let mut tx=db.begin(); tx.insert(b"a",b"old").unwrap(); let lsn=tx.commit().unwrap();
        let usage=db.history_usage(); let before=std::fs::read(dir.path().join("rows.wal")).unwrap();
        let mut tx=db.begin(); tx.replace(b"a",b"new").unwrap(); tx.insert(b"b",b"new").unwrap();
        quota(tx.commit().unwrap_err(),resource);
        assert_eq!(db.history_usage(),usage); assert_eq!(db.latest_lsn(),lsn);
        assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(),before);
        assert_eq!(db.read_as_of(b"a",lsn).unwrap(),Some(b"old".to_vec()));
        assert_eq!(db.read_as_of(b"b",lsn).unwrap(),None); assert_eq!(db.stats().active_snapshots,0);
    }
}

#[test]
fn exact_usage_including_tombstones_survives_checkpoint_and_lower_policy_refusal() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap();
    let mut tx=db.begin(); tx.insert(b"a",b"one").unwrap(); tx.commit().unwrap();
    let mut tx=db.begin(); tx.replace(b"a",b"two").unwrap(); tx.commit().unwrap();
    let mut tx=db.begin(); tx.delete(b"a").unwrap(); tx.commit().unwrap();
    let mut tx=db.begin(); tx.insert(b"b",b"three").unwrap(); tx.commit().unwrap();
    let usage=db.history_usage(); assert_eq!(usage.keys,2); assert_eq!(usage.versions,4);
    assert_eq!(db.stats().persisted_versions,usage.versions); db.checkpoint().unwrap(); drop(db);
    let exact=limits(usage.keys,usage.versions,usage.bytes);
    let reopened=DurableMvccStore::open_with_limits(dir.path(),exact).unwrap(); assert_eq!(reopened.history_usage(),usage); drop(reopened);
    let before=std::fs::read(dir.path().join("rows.wal")).unwrap();
    for (policy,resource) in [(limits(1,4,usage.bytes),"keys"),(limits(2,3,usage.bytes),"versions"),(limits(2,4,usage.bytes-1),"bytes")] {
        quota(DurableMvccStore::open_with_limits(dir.path(),policy).err().unwrap(),resource);
        assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(),before);
    }
    let reopened=DurableMvccStore::open(dir.path()).unwrap(); assert_eq!(reopened.history_usage(),usage);
    assert_eq!(reopened.read_as_of(b"a",reopened.latest_lsn()).unwrap(),None);
}

#[test]
fn explicit_retention_refunds_committed_versions_but_respects_old_snapshot_pins() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open_with_limits(dir.path(),limits(1,3,10000)).unwrap();
    let mut tx=db.begin(); tx.insert(b"a",b"a").unwrap(); tx.commit().unwrap(); let pin=db.begin();
    for value in [b"bb".as_slice(),b"ccc"] { let mut tx=db.begin(); tx.replace(b"a",value).unwrap(); tx.commit().unwrap(); }
    let floor=db.latest_lsn(); let full=db.history_usage();
    let mut tx=db.begin(); tx.replace(b"a",b"next").unwrap(); quota(tx.commit().unwrap_err(),"versions");
    assert!(matches!(db.prune_through(floor),Err(DurableMvccError::PinnedSnapshot))); assert_eq!(db.history_usage(),full);
    drop(pin); assert_eq!(db.prune_through(floor).unwrap(),2);
    let reduced=db.history_usage(); assert_eq!(reduced.keys,1); assert_eq!(reduced.versions,1); assert!(reduced.bytes<full.bytes);
    let expected_dir=tempfile::tempdir().unwrap(); let expected=DurableMvccStore::open(expected_dir.path()).unwrap();
    let mut tx=expected.begin(); tx.insert(b"a",b"ccc").unwrap(); tx.commit().unwrap(); assert_eq!(reduced,expected.history_usage());
    let mut tx=db.begin(); tx.replace(b"a",b"next").unwrap(); tx.commit().unwrap(); assert_eq!(db.history_usage().versions,2);
}

#[test]
fn interrupted_prune_prefix_rebuilds_exact_usage_and_resumes_bounded_batches() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap();
    let mut tx=db.begin(); tx.insert(b"hot",b"v").unwrap(); tx.commit().unwrap();
    for _ in 0..599 { let mut tx=db.begin(); tx.replace(b"hot",b"v").unwrap(); tx.commit().unwrap(); }
    let floor=db.latest_lsn(); assert_eq!(db.prune_through(floor).unwrap(),599); let final_usage=db.history_usage(); drop(db);
    // Construct the legal durable prefix before the final deletion batch. No
    // claim is made that acknowledged writes can disappear on qualified storage.
    let path=dir.path().join("rows.wal"); let bytes=std::fs::read(&path).unwrap(); let mut at=0; let mut last=0;
    while at<bytes.len() { last=at; assert_eq!(&bytes[at..at+4],b"BRWL"); at+=12+u32::from_le_bytes(bytes[at+4..at+8].try_into().unwrap()) as usize; }
    assert_eq!(at,bytes.len()); let file=std::fs::OpenOptions::new().write(true).open(path).unwrap(); file.set_len(last as u64).unwrap(); file.sync_all().unwrap(); drop(file);
    let db=DurableMvccStore::open(dir.path()).unwrap(); assert_eq!(db.stats().retention_floor,floor);
    assert_eq!(db.history_usage().versions,88); assert_eq!(db.stats().persisted_versions,88);
    assert_eq!(db.prune_through(floor).unwrap(),87); assert_eq!(db.history_usage(),final_usage);
    assert_eq!(db.read_as_of(b"hot",floor).unwrap(),Some(b"v".to_vec())); db.checkpoint().unwrap(); drop(db);
    assert_eq!(DurableMvccStore::open(dir.path()).unwrap().history_usage(),final_usage);
}

#[test]
fn concurrent_disjoint_writers_cannot_over_admit_one_remaining_version_slot() {
    let dir=tempfile::tempdir().unwrap(); let db=Arc::new(DurableMvccStore::open_with_limits(dir.path(),limits(2,1,10000)).unwrap());
    let barrier=Arc::new(Barrier::new(2)); let mut threads=Vec::new();
    for key in [b"a",b"b"] { let db=Arc::clone(&db); let barrier=Arc::clone(&barrier); threads.push(std::thread::spawn(move || {
        let mut tx=db.begin(); tx.insert(key,b"v").unwrap(); barrier.wait(); tx.commit()
    })); }
    let results=threads.into_iter().map(|t|t.join().unwrap()).collect::<Vec<_>>(); assert_eq!(results.iter().filter(|r|r.is_ok()).count(),1);
    for error in results.into_iter().filter_map(Result::err) { quota(error,"versions"); }
    assert_eq!(db.history_usage().versions,1); assert_eq!(db.history_usage().keys,1); assert_eq!(db.stats().active_snapshots,0);
}

#[test]
fn savepoints_and_coalesced_writes_charge_only_the_final_committed_key_set() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open_with_limits(dir.path(),limits(1,2,10000)).unwrap();
    let mut tx=db.begin(); tx.savepoint("empty").unwrap(); tx.insert(b"discarded",b"x").unwrap(); tx.rollback_to("empty").unwrap();
    tx.insert(b"kept",b"v").unwrap(); tx.release_savepoint("empty").unwrap(); tx.commit().unwrap(); assert_eq!(db.history_usage().versions,1);
    let mut tx=db.begin(); tx.replace(b"kept",b"longer").unwrap(); tx.delete(b"kept").unwrap(); tx.commit().unwrap();
    assert_eq!(db.history_usage().versions,2); db.prune_through(db.latest_lsn()).unwrap();
    assert_eq!(db.history_usage().keys,1); assert_eq!(db.history_usage().versions,1);
    let mut tx=db.begin(); tx.insert(b"new",b"v").unwrap(); quota(tx.commit().unwrap_err(),"keys");
}

#[test]
fn empty_policy_allows_format_markers_but_no_user_history() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open_with_limits(dir.path(),limits(0,0,0)).unwrap();
    assert_eq!(db.history_usage(),HistoryUsage::default()); let lsn=db.latest_lsn();
    let mut tx=db.begin(); tx.insert(b"a",b"v").unwrap(); quota(tx.commit().unwrap_err(),"keys");
    assert_eq!(db.latest_lsn(),lsn); assert_eq!(db.history_usage(),HistoryUsage::default());
}

#[test]
fn prune_cursor_crosses_batch_and_key_boundaries_without_losing_tombstone_baselines() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open_with_limits(dir.path(),limits(100,600,1000000)).unwrap();
    let keys=(0_u32..100).map(|id|id.to_be_bytes().to_vec()).collect::<Vec<_>>();
    let mut tx=db.begin(); for key in &keys { tx.insert(key,b"0").unwrap(); } tx.commit().unwrap();
    for round in 1..=5_u8 {
        let mut tx=db.begin();
        for (id,key) in keys.iter().enumerate() {
            if round==5 && id%2==0 { tx.delete(key).unwrap(); }
            else { tx.replace(key,&[round]).unwrap(); }
        }
        tx.commit().unwrap();
    }
    assert_eq!(db.history_usage().versions,600); let floor=db.latest_lsn();
    assert_eq!(db.prune_through(floor).unwrap(),500);
    assert_eq!(db.history_usage().keys,100); assert_eq!(db.history_usage().versions,100);
    for (id,key) in keys.iter().enumerate() {
        assert_eq!(db.read_as_of(key,floor).unwrap(),if id%2==0 { None } else { Some(vec![5]) });
    }
    let usage=db.history_usage(); db.checkpoint().unwrap(); drop(db);
    let raw=bradian_relational::row_store::RowStore::open(dir.path()).unwrap();
    assert_eq!(raw.len(),usage.versions+2); // history records plus format/floor markers
    drop(raw);
    assert_eq!(DurableMvccStore::open(dir.path()).unwrap().history_usage(),usage);
}
