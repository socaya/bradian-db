use std::fs;
use bradian_relational::row_store::{RowStore,StoreError};
use bradian_relational::btree::{DiskBTree,IndexError,RowPointer};
use bradian_relational::runtime_limits::{IndexBuildLimits,ResourceError,StorageLimits};
fn cost(key:&[u8])->usize {
    key.len()+std::mem::size_of::<(Vec<u8>,RowPointer)>()+2*std::mem::size_of::<(u64,&[u8])>()
}
fn quota(err:StoreError,resource:&str) {
    assert!(matches!(err,StoreError::Resource(ResourceError::IndexBuildLimit { resource:actual,.. }) if actual==resource));
}
fn policy(entries:usize,bytes:usize)->IndexBuildLimits { IndexBuildLimits { max_entries:entries,max_bytes:bytes } }

#[test]
fn checkpoint_refusal_precedes_quarantine_temporary_writes_and_wal_changes() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"old").unwrap(); db.checkpoint().unwrap();
    db.put(b"a",b"new").unwrap(); db.put(b"b",b"v").unwrap(); let lsn=db.last_lsn(); let usage=db.resource_usage();
    let wal=fs::read(dir.path().join("rows.wal")).unwrap(); let manifest=fs::read(dir.path().join("checkpoint.current")).unwrap();
    let tmp=dir.path().join(format!("snapshot-{lsn}.tmp")); let orphan=dir.path().join(format!("snapshot-{lsn}.brp"));
    fs::write(&tmp,b"prior temporary").unwrap(); fs::write(&orphan,b"prior unpublished").unwrap();
    for (limits,resource) in [(policy(1,usize::MAX),"entries"),(policy(2,0),"bytes")] {
        quota(db.checkpoint_with_limits(limits).unwrap_err(),resource);
        assert_eq!(fs::read(&tmp).unwrap(),b"prior temporary"); assert_eq!(fs::read(&orphan).unwrap(),b"prior unpublished");
        assert_eq!(fs::read(dir.path().join("rows.wal")).unwrap(),wal); assert_eq!(fs::read(dir.path().join("checkpoint.current")).unwrap(),manifest);
        assert_eq!(db.last_lsn(),lsn); assert_eq!(db.resource_usage(),usage); assert_eq!(db.get(b"a"),Some(b"new".to_vec()));
    }
    db.put(b"healthy",b"yes").unwrap(); db.checkpoint().unwrap(); assert_eq!(db.uncheckpointed_index_changes(),0);
}

#[test]
fn exact_pointer_and_descriptor_reservation_admits_checkpoint_and_idempotent_retry() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); db.put(b"bb",b"v").unwrap();
    let exact=cost(b"a")+cost(b"bb"); quota(db.checkpoint_with_limits(policy(2,exact-1)).unwrap_err(),"bytes");
    let lsn=db.checkpoint_with_limits(policy(2,exact)).unwrap(); assert_eq!(db.checkpoint_with_limits(policy(0,0)).unwrap(),lsn);
    assert_eq!(db.verify_checkpoint_index().unwrap().rows,2); drop(db);
    let db=RowStore::open(dir.path()).unwrap(); assert_eq!(db.indexed_get(b"bb").unwrap(),Some(b"v".to_vec()));
}

#[test]
fn direct_builder_admission_precedes_file_creation_and_preserves_compatible_bytes() {
    let dir=tempfile::tempdir().unwrap(); let entries=vec![(b"a".to_vec(),RowPointer { page_id:1,offset:0 })];
    for (limits,resource) in [(policy(0,1000),"entries"),(policy(1,cost(b"a")-1),"bytes")] {
        let target=dir.path().join("refused.bri");
        assert!(matches!(DiskBTree::build_with_limits(&target,7,&entries,limits),Err(IndexError::Resource(ResourceError::IndexBuildLimit { resource:actual,.. })) if actual==resource)); assert!(!target.exists());
    }
    let bounded=dir.path().join("bounded.bri"); let normal=dir.path().join("normal.bri");
    DiskBTree::build_with_limits(&bounded,7,&entries,policy(1,cost(b"a"))).unwrap(); DiskBTree::build(&normal,7,&entries).unwrap();
    assert_eq!(fs::read(&bounded).unwrap(),fs::read(normal).unwrap()); assert_eq!(DiskBTree::open(&bounded,7).unwrap().get(b"a").unwrap(),Some(entries[0].1));
    assert!(DiskBTree::build_with_limits(&bounded,7,&entries,policy(0,0)).is_err());
}

#[test]
fn owner_policy_is_retained_while_operation_override_remains_local() {
    let dir=tempfile::tempdir().unwrap(); let policies=StorageLimits { build:policy(1,10000),..StorageLimits::default() };
    let db=RowStore::open_with_policies(dir.path(),policies).unwrap(); db.put(b"a",b"v").unwrap(); db.put(b"b",b"v").unwrap();
    quota(db.checkpoint().unwrap_err(),"entries"); db.checkpoint_with_limits(policy(2,10000)).unwrap();
    assert_eq!(db.storage_limits().build.max_entries,1); db.put(b"c",b"v").unwrap(); quota(db.checkpoint().unwrap_err(),"entries");
    assert_eq!(db.uncheckpointed_index_changes(),1); db.checkpoint_with_limits(policy(3,10000)).unwrap();
}

#[test]
fn automatic_rebuild_build_refusal_preserves_prior_temp_and_incomplete_tail() {
    use std::io::Write;
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); let lsn=db.checkpoint().unwrap(); drop(db);
    let index=dir.path().join(format!("index-{lsn}.bri")); fs::remove_file(&index).unwrap();
    let tmp=dir.path().join(format!("index-{lsn}.rebuild.tmp")); fs::write(&tmp,b"prior temporary").unwrap();
    let walpath=dir.path().join("rows.wal"); fs::OpenOptions::new().append(true).open(&walpath).unwrap().write_all(b"BRW").unwrap(); let wal=fs::read(&walpath).unwrap();
    let low=StorageLimits { build:policy(0,0),..StorageLimits::default() };
    quota(RowStore::open_with_policies(dir.path(),low).unwrap_err(),"entries"); assert!(!index.exists()); assert_eq!(fs::read(&tmp).unwrap(),b"prior temporary"); assert_eq!(fs::read(&walpath).unwrap(),wal);
    let db=RowStore::open_with_policies(dir.path(),StorageLimits { build:policy(1,cost(b"a")),..low }).unwrap();
    assert_eq!(db.verify_checkpoint_index().unwrap().rows,1); assert_eq!(fs::metadata(walpath).unwrap().len(),0);
}

#[test]
fn borrowed_descriptors_preserve_multilevel_binary_key_index_and_rebuild() {
    let dir=tempfile::tempdir().unwrap(); let keys=(0_u32..80).map(|n| { let mut k=n.to_be_bytes().to_vec(); k.resize(512,0xff); k }).collect::<Vec<_>>();
    let policies=StorageLimits { build:policy(keys.len(),keys.iter().map(|k|cost(k)).sum()),..StorageLimits::default() };
    let db=RowStore::open_with_policies(dir.path(),policies).unwrap(); for key in &keys { db.put(key,b"v").unwrap(); } let lsn=db.checkpoint().unwrap();
    for key in &keys { assert_eq!(db.indexed_get(key).unwrap(),Some(b"v".to_vec())); }
    assert_eq!(db.verify_checkpoint_index().unwrap().rows,80); drop(db);
    fs::remove_file(dir.path().join(format!("index-{lsn}.bri"))).unwrap(); let db=RowStore::open_with_policies(dir.path(),policies).unwrap();
    assert_eq!(db.verify_checkpoint_index().unwrap().rows,80); assert_eq!(db.indexed_range(&[],&[0xff;513]).unwrap().len(),80);
}

#[test]
fn composed_policies_round_trip_backup_restore_without_persisting_limits() {
    let dir=tempfile::tempdir().unwrap(); let source=dir.path().join("source"); let db=RowStore::open(&source).unwrap(); db.put(b"a",b"v").unwrap(); db.checkpoint().unwrap(); drop(db);
    let policies=StorageLimits { build:policy(0,0),..StorageLimits::default() }; let db=RowStore::open_with_policies(&source,policies).unwrap();
    let archive=dir.path().join("archive"); db.backup_current_generation(&archive).unwrap(); let dest=dir.path().join("restored");
    RowStore::restore_backup_with_policies(&archive,&dest,policies).unwrap();
    let restored=RowStore::open_with_policies(&dest,policies).unwrap(); assert_eq!(restored.verify_checkpoint_index().unwrap().rows,1);
    restored.put(b"b",b"v").unwrap(); quota(restored.checkpoint().unwrap_err(),"entries"); drop(restored);
    let restored=RowStore::open(&dest).unwrap(); restored.checkpoint().unwrap(); assert_eq!(restored.verify_checkpoint_index().unwrap().rows,2);
}

#[test]
fn empty_checkpoint_and_empty_builder_fit_zero_reservation() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open_with_policies(dir.path(),StorageLimits { build:policy(0,0),..StorageLimits::default() }).unwrap();
    assert_eq!(db.checkpoint().unwrap(),0); assert_eq!(db.verify_checkpoint_index().unwrap().rows,0);
    db.put(b"a",b"v").unwrap(); quota(db.checkpoint().unwrap_err(),"entries");
    let empty=dir.path().join("empty.bri"); DiskBTree::build_with_limits(&empty,0,&[],policy(0,0)).unwrap(); assert!(DiskBTree::open(&empty,0).unwrap().is_empty());
}
