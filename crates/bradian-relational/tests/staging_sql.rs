#[path="support/epoch_fixture.rs"]
mod fixture;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::runtime_limits::{ScanControl,StagingLimits,ResourceError};
use bradian_relational::sql::SqlError;
use bradian_relational::transaction_domain::DomainError;
use bradian_relational::typed_table::Value;
fn quota(err:SqlError) { assert!(matches!(err,SqlError::Domain(DomainError::Resource(ResourceError::StagingLimit { .. })))); }

#[test]
fn statement_staging_refusal_without_markers_aborts_all_tables_and_releases_pin() {
    let (_dir,root)=fixture::migrated(); let db=HistorySqlEngine::open(&root).unwrap(); let lsn=fixture::modify(&db,&["UPDATE bank.public.accounts SET balance=150 WHERE id=1"]);
    db.with_session_limits(ScanControl::default(),StagingLimits { max_keys:1,max_bytes:10000 },|s| {
        s.execute("BEGIN")?; s.execute("UPDATE bank.public.accounts SET balance=101 WHERE id=1")?;
        quota(s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9").unwrap_err());
        assert!(!s.in_transaction()); assert!(s.staging_usage().is_none()); assert!(s.execute("COMMIT").is_err()); Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(),lsn); assert_eq!(db.mvcc_stats().active_snapshots,0);
    assert_eq!(fixture::query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(150)]]);
}

#[test]
fn refused_savepoint_copy_requires_explicit_recovery_and_preserves_prior_marker() {
    let (_dir,root)=fixture::migrated(); let db=HistorySqlEngine::open(&root).unwrap();
    let exact=db.with_session(|s| { s.execute("BEGIN")?; s.execute("UPDATE bank.public.accounts SET balance=101 WHERE id=1")?;
        s.execute("SAVEPOINT s")?; let n=s.staging_usage().unwrap().bytes; s.execute("ROLLBACK")?; Ok(n) }).unwrap();
    db.with_session_limits(ScanControl::default(),StagingLimits { max_keys:256,max_bytes:exact },|s| {
        s.execute("BEGIN WITH READ VALIDATION")?; s.execute("UPDATE bank.public.accounts SET balance=101 WHERE id=1")?;
        s.execute("SAVEPOINT s")?; quota(s.execute("SAVEPOINT child").unwrap_err()); assert!(s.in_transaction());
        assert!(s.execute("COMMIT").is_err()); assert!(s.execute("SELECT id FROM bank.public.accounts").is_err());
        s.execute("ROLLBACK TO s")?; assert!(s.read_validation_enabled()); assert_eq!(s.staging_usage().unwrap().bytes,exact);
        s.execute("RELEASE s")?; s.execute("UPDATE bank.public.accounts SET balance=102 WHERE id=1")?; s.execute("COMMIT")?; Ok(())
    }).unwrap();
    assert_eq!(fixture::query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(102)]]);
    assert_eq!(db.mvcc_stats().active_snapshots,0); drop(db);
    let db=HistorySqlEngine::open(&root).unwrap(); assert_eq!(fixture::query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(102)]]);
}

#[test]
fn zero_byte_sql_policy_preserves_read_only_queries_but_refuses_writes() {
    let (_dir,root)=fixture::migrated(); let db=HistorySqlEngine::open(&root).unwrap(); let before=db.last_lsn();
    db.with_session_limits(ScanControl::default(),StagingLimits { max_keys:256,max_bytes:0 },|s| {
        s.execute("BEGIN")?; s.execute("SELECT id FROM bank.public.accounts")?; s.execute("COMMIT")?;
        s.execute("BEGIN")?; quota(s.execute("UPDATE bank.public.accounts SET balance=999 WHERE id=1").unwrap_err());
        assert!(!s.in_transaction()); Ok(())
    }).unwrap(); assert_eq!(db.last_lsn(),before); assert_eq!(db.mvcc_stats().active_snapshots,0);
}
