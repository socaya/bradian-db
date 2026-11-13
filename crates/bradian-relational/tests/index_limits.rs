use std::fs;
use bradian_relational::row_store::{RowStore,StoreError};
use bradian_relational::runtime_limits::{IndexLimits,ResourceError};
use bradian_relational::btree::RowPointer;
use bradian_relational::page::{Page,PAGE_SIZE};
fn quota(err:StoreError,resource:&str) {
    assert!(matches!(err,StoreError::Resource(ResourceError::IndexLimit { resource:actual,.. }) if actual==resource));
}
fn cost(key:&[u8])->usize { key.len()+std::mem::size_of::<(Vec<u8>,RowPointer)>() }

#[test]
fn exact_pointer_budget_is_independent_of_checkpoint_value_lengths() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap();
    db.put(b"a",&vec![9;3500]).unwrap(); let lsn=db.checkpoint().unwrap(); let path=dir.path().join(format!("index-{lsn}.bri")); let index=fs::read(&path).unwrap();
    let limits=IndexLimits { max_entries:1,max_bytes:cost(b"a") };
    assert_eq!(db.verify_checkpoint_index_with_limits(limits).unwrap().rows,1);
    quota(db.verify_checkpoint_index_with_limits(IndexLimits { max_entries:0,..limits }).unwrap_err(),"entries");
    quota(db.verify_checkpoint_index_with_limits(IndexLimits { max_bytes:limits.max_bytes-1,..limits }).unwrap_err(),"bytes");
    assert_eq!(fs::read(path).unwrap(),index); assert_eq!(db.get(b"a"),Some(vec![9;3500]));
    db.put(b"still-healthy",b"v").unwrap();
}

#[test]
fn refused_rebuild_leaves_authority_and_prior_temp_untouched_then_higher_policy_recovers() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap();
    db.put(b"a",b"one").unwrap(); db.put(b"b",b"two").unwrap(); let lsn=db.checkpoint().unwrap(); db.put(b"later",b"wal").unwrap();
    let snapshot=dir.path().join(format!("snapshot-{lsn}.brp")); let wal=dir.path().join("rows.wal");
    let before_snapshot=fs::read(&snapshot).unwrap(); let before_wal=fs::read(&wal).unwrap();
    let index=dir.path().join(format!("index-{lsn}.bri")); fs::remove_file(&index).unwrap();
    let tmp=dir.path().join(format!("index-{lsn}.rebuild.tmp")); fs::write(&tmp,b"prior attempt").unwrap();
    for limits in [IndexLimits { max_entries:1,max_bytes:usize::MAX },IndexLimits { max_entries:2,max_bytes:0 }] {
        assert!(matches!(db.rebuild_missing_checkpoint_index_with_limits(limits),Err(StoreError::Resource(ResourceError::IndexLimit { .. }))));
        assert!(!index.exists()); assert_eq!(fs::read(&tmp).unwrap(),b"prior attempt");
        assert_eq!(fs::read(&snapshot).unwrap(),before_snapshot); assert_eq!(fs::read(&wal).unwrap(),before_wal);
    }
    let limits=IndexLimits { max_entries:2,max_bytes:cost(b"a")+cost(b"b") };
    assert!(db.rebuild_missing_checkpoint_index_with_limits(limits).unwrap().rebuilt);
    assert!(!db.rebuild_missing_checkpoint_index_with_limits(limits).unwrap().rebuilt);
    assert_eq!(db.verify_checkpoint_index_with_limits(limits).unwrap().rows,2);
    assert_eq!(db.indexed_get(b"later").unwrap(),Some(b"wal".to_vec()));
    assert_eq!(fs::read(snapshot).unwrap(),before_snapshot); assert_eq!(fs::read(wal).unwrap(),before_wal);
}

#[test]
fn zero_policy_accepts_empty_audit_and_empty_rebuild() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.checkpoint().unwrap();
    let zero=IndexLimits { max_entries:0,max_bytes:0 }; assert_eq!(db.verify_checkpoint_index_with_limits(zero).unwrap().rows,0);
    fs::remove_file(dir.path().join("index-0.bri")).unwrap();
    assert!(db.rebuild_missing_checkpoint_index_with_limits(zero).unwrap().rebuilt);
    assert_eq!(db.verify_checkpoint_index_with_limits(zero).unwrap().rows,0);
}

fn long_keys()->Vec<Vec<u8>> {
    (0_u32..80).map(|n|{ let mut key=n.to_be_bytes().to_vec(); key.resize(512,0xff); key }).collect()
}
#[test]
fn streaming_reconciliation_covers_multiple_levels_and_full_binary_key_range() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); let keys=long_keys();
    for key in &keys { db.put(key,b"value").unwrap(); } let lsn=db.checkpoint().unwrap();
    let limits=IndexLimits { max_entries:keys.len(),max_bytes:keys.iter().map(|k|cost(k)).sum() };
    assert_eq!(db.verify_checkpoint_index_with_limits(limits).unwrap().rows,80);
    fs::remove_file(dir.path().join(format!("index-{lsn}.bri"))).unwrap();
    assert!(db.rebuild_missing_checkpoint_index_with_limits(limits).unwrap().rebuilt);
    for key in &keys { assert_eq!(db.indexed_get(key).unwrap(),Some(b"value".to_vec())); }
    assert_eq!(db.indexed_range(&[],&[0xff;513]).unwrap().len(),80);
}

#[test]
fn valid_crc_with_premature_leaf_chain_end_is_rejected_without_overwrite() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap();
    for key in long_keys() { db.put(&key,b"v").unwrap(); } let lsn=db.checkpoint().unwrap();
    let index=dir.path().join(format!("index-{lsn}.bri")); let mut bytes=fs::read(&index).unwrap();
    let mut leaf=Page::decode(&bytes[PAGE_SIZE..2*PAGE_SIZE]).unwrap();
    assert_ne!(&leaf.payload[3..11],&0_u64.to_le_bytes()); leaf.payload[3..11].copy_from_slice(&0_u64.to_le_bytes());
    bytes[PAGE_SIZE..2*PAGE_SIZE].copy_from_slice(&leaf.encode().unwrap()); fs::write(&index,&bytes).unwrap();
    assert!(db.verify_checkpoint_index().is_err()); assert!(db.rebuild_missing_checkpoint_index().is_err()); assert_eq!(fs::read(index).unwrap(),bytes);
}

#[test]
fn valid_crc_impossible_leaf_count_is_rejected_before_entry_reservation() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); let lsn=db.checkpoint().unwrap();
    let index=dir.path().join(format!("index-{lsn}.bri")); let mut bytes=fs::read(&index).unwrap(); let mut leaf=Page::decode(&bytes[PAGE_SIZE..2*PAGE_SIZE]).unwrap();
    leaf.payload[1..3].copy_from_slice(&u16::MAX.to_le_bytes()); bytes[PAGE_SIZE..2*PAGE_SIZE].copy_from_slice(&leaf.encode().unwrap()); fs::write(&index,&bytes).unwrap();
    assert!(db.verify_checkpoint_index().is_err()); assert!(db.indexed_get(b"a").is_err()); assert_eq!(fs::read(index).unwrap(),bytes);
}

#[test]
fn checkpoint_row_structure_still_validated_when_values_are_not_retained() {
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"value").unwrap(); let lsn=db.checkpoint().unwrap();
    let path=dir.path().join(format!("snapshot-{lsn}.brp")); let mut bytes=fs::read(&path).unwrap(); let mut page=Page::decode(&bytes[PAGE_SIZE..2*PAGE_SIZE]).unwrap();
    page.payload[2..6].copy_from_slice(&u32::MAX.to_le_bytes()); bytes[PAGE_SIZE..2*PAGE_SIZE].copy_from_slice(&page.encode().unwrap()); fs::write(&path,&bytes).unwrap();
    assert!(matches!(db.verify_checkpoint_index(),Err(StoreError::Corruption(_))));
    let index=dir.path().join(format!("index-{lsn}.bri")); fs::remove_file(&index).unwrap();
    assert!(db.rebuild_missing_checkpoint_index().is_err()); assert!(!index.exists()); assert_eq!(fs::read(path).unwrap(),bytes);
}

#[test]
fn open_policy_refusal_preserves_wal_tail_and_missing_index_until_raised() {
    use std::io::Write;
    use bradian_relational::runtime_limits::RowStoreLimits;
    let dir=tempfile::tempdir().unwrap(); let db=RowStore::open(dir.path()).unwrap(); db.put(b"a",b"v").unwrap(); db.put(b"b",b"v").unwrap(); let lsn=db.checkpoint().unwrap(); drop(db);
    let index=dir.path().join(format!("index-{lsn}.bri")); fs::remove_file(&index).unwrap(); let walpath=dir.path().join("rows.wal");
    fs::OpenOptions::new().append(true).open(&walpath).unwrap().write_all(b"BRW").unwrap(); let wal=fs::read(&walpath).unwrap();
    let low=IndexLimits { max_entries:1,max_bytes:10000 };
    quota(RowStore::open_with_recovery_limits(dir.path(),RowStoreLimits::default(),low).unwrap_err(),"entries");
    assert!(!index.exists()); assert_eq!(fs::read(&walpath).unwrap(),wal);
    let db=RowStore::open_with_recovery_limits(dir.path(),RowStoreLimits::default(),IndexLimits { max_entries:2,..low }).unwrap();
    assert!(index.exists()); assert_eq!(fs::metadata(walpath).unwrap().len(),0); assert_eq!(db.verify_checkpoint_index().unwrap().rows,2);
}

#[test]
fn backup_and_restore_propagate_selected_index_policy_before_publication() {
    use bradian_relational::runtime_limits::RowStoreLimits;
    let dir=tempfile::tempdir().unwrap(); let source=dir.path().join("source"); let db=RowStore::open(&source).unwrap();
    db.put(b"a",b"v").unwrap(); db.put(b"b",b"v").unwrap(); db.checkpoint().unwrap(); drop(db);
    let low=IndexLimits { max_entries:1,max_bytes:10000 }; let high=IndexLimits { max_entries:2,..low };
    let db=RowStore::open_with_recovery_limits(&source,RowStoreLimits::default(),low).unwrap();
    let refused=dir.path().join("refused-backup"); quota(db.backup_current_generation(&refused).unwrap_err(),"entries"); assert!(!refused.exists()); drop(db);
    let db=RowStore::open_with_recovery_limits(&source,RowStoreLimits::default(),high).unwrap();
    let archive=dir.path().join("archive"); db.backup_current_generation(&archive).unwrap(); let before=fs::read(archive.join("rows.wal")).unwrap();
    let refused=dir.path().join("refused-restore"); quota(RowStore::restore_backup_with_recovery_limits(&archive,&refused,RowStoreLimits::default(),low).unwrap_err(),"entries"); assert!(!refused.exists());
    assert_eq!(fs::read(archive.join("rows.wal")).unwrap(),before);
    let restored=dir.path().join("restored"); RowStore::restore_backup_with_recovery_limits(&archive,&restored,RowStoreLimits::default(),high).unwrap();
    let restored=RowStore::open_with_recovery_limits(&restored,RowStoreLimits::default(),high).unwrap(); assert_eq!(restored.verify_checkpoint_index().unwrap().rows,2);
}
