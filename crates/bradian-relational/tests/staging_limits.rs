use bradian_relational::durable_mvcc::{DurableMvccError,DurableMvccStore};
use bradian_relational::runtime_limits::{ResourceError,StagingLimits,StagingUsage};

fn policy(keys:usize,bytes:usize)->StagingLimits { StagingLimits { max_keys:keys,max_bytes:bytes } }
fn quota(err:DurableMvccError,resource:&str) {
    assert!(matches!(err,DurableMvccError::Resource(ResourceError::StagingLimit { resource:actual,.. }) if actual==resource));
}

#[test]
fn physical_key_ceiling_is_enforced_before_copy_even_with_higher_policy() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap(); let lsn=db.latest_lsn();
    let mut tx=db.begin_with_limits(policy(usize::MAX,usize::MAX),false);
    for n in 0_u32..256 { tx.insert(&n.to_be_bytes(),b"v").unwrap(); }
    let usage=tx.staging_usage(); let wal=std::fs::read(dir.path().join("rows.wal")).unwrap();
    quota(tx.insert(&256_u32.to_be_bytes(),b"v").unwrap_err(),"keys");
    assert_eq!(tx.staging_usage(),usage); assert_eq!(tx.get(&256_u32.to_be_bytes()).unwrap(),None);
    assert_eq!(db.latest_lsn(),lsn); assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(),wal);
    tx.commit().unwrap(); assert_eq!(db.history_usage().keys,256); assert_eq!(db.stats().active_snapshots,0);
}

#[test]
fn exact_byte_boundary_and_replacement_refunds_preserve_refused_rows() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap();
    let mut probe=db.begin(); probe.insert(b"a",b"1234").unwrap(); let exact=probe.staging_usage().bytes; probe.rollback();
    let mut tx=db.begin_with_limits(policy(1,exact),false); tx.insert(b"a",b"1234").unwrap();
    quota(tx.replace(b"a",b"12345").unwrap_err(),"bytes");
    assert_eq!(tx.get(b"a").unwrap(),Some(b"1234".to_vec())); assert_eq!(tx.staging_usage().bytes,exact);
    tx.replace(b"a",b"12").unwrap(); assert_eq!(tx.staging_usage().bytes,exact-2);
    tx.delete(b"a").unwrap(); assert_eq!(tx.staging_usage().bytes,exact-4); assert_eq!(tx.staging_usage().keys,1);
    tx.insert(b"a",b"1234").unwrap(); quota(tx.insert(b"b",b"").unwrap_err(),"keys");
    tx.commit().unwrap(); assert_eq!(db.history_usage().versions,1);
}

#[test]
fn savepoints_share_one_budget_and_release_refunds_copied_maps_and_markers() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap();
    let mut probe=db.begin(); probe.insert(b"a",b"value").unwrap(); let row=probe.staging_usage();
    probe.savepoint("s").unwrap(); let exact=probe.staging_usage().bytes; probe.rollback();
    let mut tx=db.begin_with_limits(policy(2,exact),false); tx.insert(b"a",b"value").unwrap(); tx.savepoint("s").unwrap();
    assert_eq!(tx.staging_usage().bytes,exact);
    quota(tx.savepoint("s").unwrap_err(),"bytes"); quota(tx.insert(b"b",b"v").unwrap_err(),"bytes");
    assert_eq!(tx.savepoint_count(),1); assert_eq!(tx.staging_usage().bytes,exact);
    tx.replace(b"a",b"v").unwrap(); tx.rollback_to("s").unwrap();
    assert_eq!(tx.get(b"a").unwrap(),Some(b"value".to_vec())); assert_eq!(tx.staging_usage().bytes,exact);
    tx.release_savepoint("s").unwrap(); assert_eq!(tx.staging_usage(),row);
    tx.savepoint("s").unwrap(); tx.rollback(); assert_eq!(db.stats().active_snapshots,0); assert_eq!(db.history_usage().versions,0);
}

#[test]
fn rollback_discards_descendants_and_restores_accounting_without_refunding_observations() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap();
    let mut initial=db.begin(); initial.insert(b"watched",b"old").unwrap(); initial.commit().unwrap();
    let mut tx=db.begin_with_limits(policy(2,10000),true);
    tx.insert(b"a",b"one").unwrap(); tx.savepoint("s").unwrap(); let target=tx.staging_usage();
    tx.get(b"watched").unwrap(); tx.replace(b"a",b"two-two").unwrap(); tx.savepoint("child").unwrap(); tx.insert(b"b",b"discard").unwrap();
    quota(tx.insert(b"c",b"overflow").unwrap_err(),"keys"); tx.rollback_to("s").unwrap();
    assert_eq!(tx.staging_usage(),target); assert_eq!(tx.savepoint_count(),1); assert_eq!(tx.get(b"b").unwrap(),None);
    let mut writer=db.begin(); writer.replace(b"watched",b"new").unwrap(); writer.commit().unwrap();
    assert!(matches!(tx.commit(),Err(DurableMvccError::ReadConflict))); assert_eq!(db.stats().active_snapshots,0);
    assert_eq!(db.read_as_of(b"a",db.latest_lsn()).unwrap(),None);
}

#[test]
fn zero_staging_policy_allows_read_only_transactions_and_no_wal_growth() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap(); let lsn=db.latest_lsn();
    let mut tx=db.begin_with_limits(policy(0,0),true); assert_eq!(tx.staging_limits().max_bytes,0);
    assert_eq!(tx.get(b"absent").unwrap(),None); quota(tx.insert(b"a",b"v").unwrap_err(),"keys");
    quota(tx.savepoint("empty").unwrap_err(),"bytes"); assert_eq!(tx.staging_usage(),StagingUsage::default());
    assert_eq!(tx.commit().unwrap(),lsn); assert_eq!(db.latest_lsn(),lsn); assert_eq!(db.stats().active_snapshots,0);
}

#[test]
fn staged_data_and_markers_are_discarded_on_drop_and_never_recovered() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap(); let lsn=db.latest_lsn();
    { let mut tx=db.begin(); tx.insert(b"a",b"not durable").unwrap(); tx.savepoint("s").unwrap(); }
    assert_eq!(db.stats().active_snapshots,0); db.checkpoint().unwrap(); drop(db);
    let db=DurableMvccStore::open(dir.path()).unwrap(); assert_eq!(db.latest_lsn(),lsn);
    assert_eq!(db.history_usage().versions,0); assert_eq!(db.begin().staging_usage(),StagingUsage::default());
}

#[test]
fn default_byte_budget_bounds_large_savepoint_copies_before_marker_ceiling() {
    let dir=tempfile::tempdir().unwrap(); let db=DurableMvccStore::open(dir.path()).unwrap();
    let mut tx=db.begin(); let value=vec![7;3000];
    for n in 0_u32..256 { tx.insert(&n.to_be_bytes(),&value).unwrap(); }
    let current=tx.staging_usage(); let mut refused=false;
    for n in 0..16 {
        let before=tx.staging_usage();
        match tx.savepoint(&format!("s{n}")) {
            Ok(())=>assert!(tx.staging_usage().bytes<=StagingLimits::default().max_bytes),
            Err(e)=>{ quota(e,"bytes"); assert_eq!(tx.staging_usage(),before); refused=true; break; }
        }
    }
    assert!(refused); assert!(tx.savepoint_count()<16);
    tx.release_savepoint("s0").unwrap(); assert_eq!(tx.staging_usage(),current);
    tx.savepoint("fresh").unwrap(); tx.rollback();
    assert_eq!(db.history_usage().versions,0); assert_eq!(db.stats().active_snapshots,0);
}
