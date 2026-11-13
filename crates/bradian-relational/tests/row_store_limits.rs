use bradian_relational::row_store::{RowStore,RowMutation,StoreError};
use bradian_relational::runtime_limits::{RowStoreLimits,RowStoreUsage,ResourceError};
use std::sync::{Arc,Barrier};
fn unlimited()->RowStoreLimits { RowStoreLimits { max_rows:usize::MAX,max_row_bytes:usize::MAX,max_delta_keys:usize::MAX,max_delta_bytes:usize::MAX } }
fn quota(err:StoreError,resource:&str) {
    assert!(matches!(err,StoreError::Resource(ResourceError::RowStoreLimit { resource:actual,.. }) if actual==resource));
}
fn put(k:&[u8],v:&[u8])->RowMutation { RowMutation::Put { key:k.to_vec(),value:v.to_vec() } }
fn delete(k:&[u8])->RowMutation { RowMutation::Delete { key:k.to_vec() } }

#[test]
fn every_map_budget_refuses_whole_batch_without_wal_lsn_or_usage_changes() {
    let probe=tempfile::tempdir().unwrap(); let db=RowStore::open(probe.path()).unwrap(); db.put(b"a",b"v").unwrap(); let single=db.resource_usage(); drop(db);
    for (limits,resource) in [
        (RowStoreLimits { max_rows:1,..unlimited() },"rows"),
        (RowStoreLimits { max_row_bytes:single.row_bytes,..unlimited() },"row bytes"),
        (RowStoreLimits { max_delta_keys:1,..unlimited() },"delta keys"),
        (RowStoreLimits { max_delta_bytes:single.delta_bytes,..unlimited() },"delta bytes"),
    ] {
        let dir=tempfile::tempdir().unwrap(); let db=RowStore::open_with_limits(dir.path(),limits).unwrap();
        db.put(b"a",b"v").unwrap(); let before=db.resource_usage(); let lsn=db.last_lsn(); let wal=std::fs::read(dir.path().join("rows.wal")).unwrap();
        quota(db.commit_batch(Some(lsn),&[put(b"a",b"x"),put(b"b",b"y")]).unwrap_err(),resource);
        assert_eq!(db.resource_usage(),before); assert_eq!(db.last_lsn(),lsn); assert_eq!(db.get(b"a"),Some(b"v".to_vec())); assert_eq!(db.get(b"b"),None);
        assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(),wal);
        db.put(b"a",b"x").unwrap(); // refusal did not poison the healthy owner
    }
}

#[test]
fn single_mutations_charge_exact_replacements_and_absent_delete_tombstones() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"1234").unwrap(); let original=db.resource_usage();
    db.put(b"a",b"12").unwrap(); let reduced=db.resource_usage();
    assert_eq!(reduced.rows,1); assert_eq!(reduced.delta_keys,1); assert_eq!(reduced.row_bytes,original.row_bytes-2); assert_eq!(reduced.delta_bytes,original.delta_bytes-2);
    db.delete(b"a").unwrap(); let tombstone=db.resource_usage(); assert_eq!(tombstone.rows,0); assert_eq!(tombstone.row_bytes,0); assert_eq!(tombstone.delta_keys,1);
    assert_eq!(tombstone.delta_bytes,original.delta_bytes-4);
    db.delete(b"b").unwrap(); let usage=db.resource_usage(); assert_eq!(usage.delta_keys,2); assert_eq!(usage.delta_bytes,2*tombstone.delta_bytes);
    let lsn=db.last_lsn(); drop(db); let db=RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:1,max_row_bytes:original.row_bytes,max_delta_keys:2,max_delta_bytes:usage.delta_bytes }).unwrap();
    assert_eq!(db.last_lsn(),lsn); assert_eq!(db.resource_usage(),usage); quota(db.delete(b"c").unwrap_err(),"delta keys");
    db.checkpoint().unwrap(); assert_eq!(db.resource_usage(),RowStoreUsage::default()); db.delete(b"c").unwrap();
}

#[test]
fn repeated_batch_keys_and_cross_key_swaps_admit_final_state_without_copy_peaks() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"z",b"v").unwrap(); db.checkpoint().unwrap(); let before=db.resource_usage(); drop(db);
    let limits=RowStoreLimits { max_rows:1,max_row_bytes:before.row_bytes,..unlimited() };
    let db=RowStore::open_with_limits(dir.path(),limits).unwrap();
    // In original order this briefly has two live rows. Its admitted final state has one.
    db.commit_batch(None,&[put(b"a",b"v"),delete(b"z"),put(b"a",b"larger"),delete(b"a"),put(b"a",b"v")]).unwrap();
    assert_eq!(db.resource_usage().rows,1); assert_eq!(db.resource_usage().row_bytes,before.row_bytes); assert_eq!(db.get(b"a"),Some(b"v".to_vec())); assert_eq!(db.get(b"z"),None);
    let usage=db.resource_usage(); let lsn=db.last_lsn(); drop(db);
    let db=RowStore::open_with_limits(dir.path(),limits).unwrap(); assert_eq!(db.resource_usage(),usage); assert_eq!(db.last_lsn(),lsn);
}

#[test]
fn checkpoint_open_checks_budgets_before_derived_index_repair_and_preserves_files() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); db.put(b"b",b"v").unwrap(); let lsn=db.checkpoint().unwrap(); let usage=db.resource_usage(); drop(db);
    let snapshot=std::fs::read(dir.path().join(format!("snapshot-{lsn}.brp"))).unwrap();
    std::fs::remove_file(dir.path().join(format!("index-{lsn}.bri"))).unwrap();
    for (limits,resource) in [(RowStoreLimits { max_rows:1,..unlimited() },"rows"),(RowStoreLimits { max_row_bytes:usage.row_bytes-1,..unlimited() },"row bytes")] {
        quota(RowStore::open_with_limits(dir.path(),limits).unwrap_err(),resource);
        assert!(!dir.path().join(format!("index-{lsn}.bri")).exists());
        assert_eq!(std::fs::read(dir.path().join(format!("snapshot-{lsn}.brp"))).unwrap(),snapshot);
    }
    let db=RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:usage.rows,max_row_bytes:usage.row_bytes,max_delta_keys:0,max_delta_bytes:0 }).unwrap();
    assert_eq!(db.resource_usage(),usage); assert!(dir.path().join(format!("index-{lsn}.bri")).exists());
}

#[test]
fn replay_refusal_preserves_complete_and_incomplete_wal_until_successful_open() {
    use std::io::Write;
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); db.checkpoint().unwrap(); db.put(b"b",b"v").unwrap(); let usage=db.resource_usage(); let lsn=db.last_lsn(); drop(db);
    let walpath=dir.path().join("rows.wal"); std::fs::OpenOptions::new().append(true).open(&walpath).unwrap().write_all(b"BRW").unwrap(); let wal=std::fs::read(&walpath).unwrap();
    for (limits,resource) in [
        (RowStoreLimits { max_rows:1,..unlimited() },"rows"),
        (RowStoreLimits { max_row_bytes:usage.row_bytes-1,..unlimited() },"row bytes"),
        (RowStoreLimits { max_delta_keys:0,..unlimited() },"delta keys"),
        (RowStoreLimits { max_delta_bytes:usage.delta_bytes-1,..unlimited() },"delta bytes"),
    ] { quota(RowStore::open_with_limits(dir.path(),limits).unwrap_err(),resource); assert_eq!(std::fs::read(&walpath).unwrap(),wal); }
    let db=RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:usage.rows,max_row_bytes:usage.row_bytes,max_delta_keys:usage.delta_keys,max_delta_bytes:usage.delta_bytes }).unwrap();
    assert_eq!(db.resource_usage(),usage); assert_eq!(db.last_lsn(),lsn); assert_eq!(std::fs::metadata(&walpath).unwrap().len() as usize,wal.len()-3);
}

#[test]
fn lower_policy_must_fit_checkpoint_and_each_replayed_commit_prefix() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); db.put(b"b",b"v").unwrap(); db.delete(b"b").unwrap(); let lsn=db.last_lsn(); drop(db);
    quota(RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:1,..unlimited() }).unwrap_err(),"rows");
    let db=RowStore::open(dir.path()).unwrap(); assert_eq!(db.last_lsn(),lsn); db.checkpoint().unwrap(); drop(db);
    let db=RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:1,..unlimited() }).unwrap(); assert_eq!(db.len(),1);
}

#[test]
fn concurrent_writers_cannot_over_admit_one_remaining_map_slot() {
    let dir=tempfile::tempdir().unwrap(); let db=Arc::new(RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:1,..unlimited() }).unwrap()); let barrier=Arc::new(Barrier::new(2));
    let handles=(0..2).map(|id| { let db=Arc::clone(&db); let barrier=Arc::clone(&barrier); std::thread::spawn(move||{ barrier.wait(); db.put(&[id],b"v") }) }).collect::<Vec<_>>();
    let results=handles.into_iter().map(|h|h.join().unwrap()).collect::<Vec<_>>(); assert_eq!(results.iter().filter(|r|r.is_ok()).count(),1);
    for err in results.into_iter().filter_map(Result::err) { quota(err,"rows"); }
    assert_eq!(db.resource_usage().rows,1); assert_eq!(db.resource_usage().delta_keys,1);
}

#[test]
fn backup_and_policy_aware_restore_never_publish_over_policy_destination() {
    let dir=tempfile::tempdir().unwrap(); let source=dir.path().join("source"); let db=RowStore::open_with_limits(&source,RowStoreLimits { max_rows:2,max_delta_keys:2,..unlimited() }).unwrap();
    db.put(b"a",b"v").unwrap(); db.checkpoint().unwrap(); db.put(b"b",b"v").unwrap(); let usage=db.resource_usage();
    let archive=dir.path().join("archive"); db.backup_current_generation(&archive).unwrap(); let before=std::fs::read(archive.join("rows.wal")).unwrap();
    let refused=dir.path().join("refused"); quota(RowStore::restore_backup_with_limits(&archive,&refused,RowStoreLimits { max_rows:1,..unlimited() }).unwrap_err(),"rows"); assert!(!refused.exists());
    assert_eq!(std::fs::read(archive.join("rows.wal")).unwrap(),before);
    let restored=dir.path().join("restored"); RowStore::restore_backup_with_limits(&archive,&restored,db.resource_limits()).unwrap();
    let restored=RowStore::open_with_limits(&restored,db.resource_limits()).unwrap(); assert_eq!(restored.resource_usage(),usage); assert_eq!(restored.last_lsn(),db.last_lsn());
}

#[test]
fn zero_policy_allows_empty_store_and_empty_checkpoint_but_no_delta_tombstone() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open_with_limits(dir.path(),RowStoreLimits { max_rows:0,max_row_bytes:0,max_delta_keys:0,max_delta_bytes:0 }).unwrap();
    quota(db.put(b"a",b"v").unwrap_err(),"rows"); quota(db.delete(b"missing").unwrap_err(),"delta keys");
    assert_eq!(db.resource_usage(),RowStoreUsage::default()); assert_eq!(db.last_lsn(),0); db.checkpoint().unwrap();
}
