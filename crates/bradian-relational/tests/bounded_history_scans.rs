use bradian_relational::durable_mvcc::{DurableMvccError, DurableMvccStore};
use bradian_relational::runtime_limits::{ResourceError, ScanControl, ScanLimits};

fn limits(rows: usize, bytes: usize, work: usize) -> ScanControl { ScanControl::new(ScanLimits { max_rows: rows, max_bytes: bytes, max_work: work }) }

#[test]
fn prefix_scans_match_full_scan_oracle_with_staged_insert_update_delete_and_history() {
    let dir = tempfile::tempdir().unwrap(); let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = store.begin();
    for (key,value) in [(b"a/1",b"one"),(b"a/2",b"two"),(b"a/3",b"old"),(b"b/1",b"out")] { tx.insert(key,value).unwrap(); }
    let before = tx.commit().unwrap();
    let mut tx = store.begin(); tx.replace(b"a/3",b"new").unwrap(); tx.delete(b"a/2").unwrap(); let after = tx.commit().unwrap();
    for horizon in [before,after] {
        let expected = store.scan_as_of(horizon).unwrap().into_iter().filter(|(key,_)|key.starts_with(b"a/")).collect::<Vec<_>>();
        assert_eq!(store.scan_prefix_as_of_with_control(b"a/",horizon,&ScanControl::default()).unwrap(),expected);
    }
    let mut tx = store.begin(); tx.replace(b"a/1",b"mine").unwrap(); tx.delete(b"a/3").unwrap(); tx.insert(b"a/4",b"four").unwrap(); tx.insert(b"b/2",b"other").unwrap();
    let expected = tx.scan().into_iter().filter(|(key,_)|key.starts_with(b"a/")).collect::<Vec<_>>();
    assert_eq!(tx.scan_prefix_with_control(b"a/",&ScanControl::default()).unwrap(),expected);
    assert!(tx.scan_prefix_with_control(b"absent",&limits(0,0,0)).unwrap().is_empty());
    tx.rollback(); assert_eq!(store.latest_lsn(),after);
}

#[test]
fn row_byte_and_work_boundaries_are_checked_before_returning_results() {
    let dir = tempfile::tempdir().unwrap(); let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx = store.begin(); tx.insert(b"a/1",b"v").unwrap();
    for i in 0_u32..200 { let mut key=b"b/".to_vec(); key.extend_from_slice(&i.to_be_bytes()); tx.insert(&key,b"unrelated").unwrap(); }
    let lsn=tx.commit().unwrap();
    let bytes=std::mem::size_of::<(Vec<u8>,Vec<u8>)>()+b"a/1".len()+1;
    assert_eq!(store.scan_prefix_as_of_with_control(b"a/",lsn,&limits(1,bytes,2)).unwrap().len(),1);
    for (control,resource) in [(limits(0,bytes,2),"rows"),(limits(1,bytes-1,2),"bytes"),(limits(1,bytes,1),"work")] {
        assert!(matches!(store.scan_prefix_as_of_with_control(b"a/",lsn,&control), Err(DurableMvccError::Resource(ResourceError::Limit { resource: actual,.. })) if actual==resource));
    }
    assert!(matches!(store.scan_prefix_as_of_with_control(&vec![0;499],lsn,&ScanControl::default()),Err(DurableMvccError::Capacity)));
    assert_eq!(store.latest_lsn(),lsn);
}

#[test]
fn historical_version_search_has_a_work_budget_even_for_one_key() {
    let dir = tempfile::tempdir().unwrap(); let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut tx=store.begin(); tx.insert(b"hot",b"0").unwrap(); let oldest=tx.commit().unwrap();
    for _ in 0..8 { let mut tx=store.begin(); tx.replace(b"hot",b"1").unwrap(); tx.commit().unwrap(); }
    let control=limits(1,1000,2);
    assert!(store.scan_prefix_as_of_with_control(b"hot",store.latest_lsn(),&control).is_ok());
    assert!(matches!(store.scan_prefix_as_of_with_control(b"hot",oldest,&control),Err(DurableMvccError::Resource(ResourceError::Limit { resource:"work",.. }))));
    assert_eq!(store.scan_prefix_as_of_with_control(b"hot",oldest,&limits(1,1000,10)).unwrap()[0].1,b"0");
}

#[test]
fn failed_partial_scan_observation_survives_savepoint_rollback_and_pins_release() {
    let dir = tempfile::tempdir().unwrap(); let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut seed=store.begin(); seed.insert(b"a/1",b"v").unwrap(); seed.insert(b"a/2",b"v").unwrap(); seed.commit().unwrap();
    let mut tx=store.begin_with_read_validation(); tx.savepoint("before").unwrap();
    assert!(matches!(tx.scan_prefix_with_control(b"a/",&limits(1,1000,100)),Err(DurableMvccError::Resource(ResourceError::Limit { resource:"rows",.. }))));
    tx.rollback_to("before").unwrap();
    let mut other=store.begin(); other.insert(b"unrelated",b"v").unwrap(); let latest=other.commit().unwrap();
    assert!(matches!(store.prune_through(latest),Err(DurableMvccError::PinnedSnapshot)));
    assert!(matches!(tx.commit(),Err(DurableMvccError::ScanConflict)));
    assert_eq!(store.stats().active_snapshots,0); assert_eq!(store.latest_lsn(),latest);
}

#[test]
fn cancelled_scan_returns_no_rows_and_does_not_write_or_leak_snapshot_pins() {
    let dir = tempfile::tempdir().unwrap(); let store = DurableMvccStore::open(dir.path()).unwrap();
    let mut seed=store.begin(); seed.insert(b"a",b"v").unwrap(); let latest=seed.commit().unwrap();
    let control=ScanControl::default(); control.cancel();
    assert!(matches!(store.scan_prefix_as_of_with_control(b"a",latest,&control),Err(DurableMvccError::Resource(ResourceError::Cancelled))));
    let mut tx=store.begin();
    assert!(matches!(tx.scan_prefix_with_control(b"a",&control),Err(DurableMvccError::Resource(ResourceError::Cancelled))));
    assert_eq!(tx.scan_prefix_with_control(b"a",&ScanControl::default()).unwrap().len(),1);
    tx.rollback(); assert_eq!(store.stats().active_snapshots,0); assert_eq!(store.latest_lsn(),latest);
}
