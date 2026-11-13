#[path="support/epoch_fixture.rs"]
mod fixture;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::runtime_limits::{HistoryLimits,TransactionLimits,ResourceError};
use bradian_relational::sql::SqlError;
use bradian_relational::transaction_domain::DomainError;
use bradian_relational::typed_table::Value;
fn quota(error:SqlError) { assert!(matches!(error,SqlError::Domain(DomainError::Resource(ResourceError::TransactionLimit { .. })))); }

#[test]
fn zero_admission_opens_valid_history_but_refuses_sql_begin_without_pins_or_writes() {
    let (_dir,root)=fixture::migrated(); let db=HistorySqlEngine::open_with_resource_limits(&root,HistoryLimits::default(),TransactionLimits { max_active:0 }).unwrap(); let lsn=db.last_lsn();
    assert_eq!(db.transaction_limits().max_active,0); assert_eq!(db.mvcc_stats().active_snapshots,0);
    db.with_session(|s|{ quota(s.execute("BEGIN").unwrap_err()); quota(s.execute("BEGIN WITH READ VALIDATION").unwrap_err()); assert!(!s.in_transaction()); Ok(()) }).unwrap();
    assert_eq!(db.last_lsn(),lsn); assert_eq!(db.mvcc_stats().active_snapshots,0); drop(db);
    let db=HistorySqlEngine::open(&root).unwrap(); fixture::modify(&db,&["UPDATE bank.public.accounts SET balance=101 WHERE id=1"]);
    assert_eq!(fixture::query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(101)]]);
}

#[test]
fn shared_sql_admission_preserves_failed_savepoint_owner_until_full_rollback() {
    let (_dir,root)=fixture::migrated(); let db=HistorySqlEngine::open_with_resource_limits(&root,HistoryLimits::default(),TransactionLimits { max_active:1 }).unwrap();
    db.with_session(|outer|{
        outer.execute("BEGIN")?; outer.execute("SAVEPOINT s")?; assert!(outer.execute("SELECT missing FROM bank.public.accounts").is_err());
        db.with_session(|inner|{ quota(inner.execute("BEGIN").unwrap_err()); assert!(!inner.in_transaction()); Ok(()) })?;
        outer.execute("ROLLBACK TO s")?; assert_eq!(db.mvcc_stats().active_snapshots,1);
        db.with_session(|inner|{ quota(inner.execute("BEGIN WITH READ VALIDATION").unwrap_err()); Ok(()) })?;
        outer.execute("ROLLBACK")?; db.with_session(|inner|{ inner.execute("BEGIN")?; inner.execute("COMMIT")?; Ok(()) })?; Ok(())
    }).unwrap(); assert_eq!(db.mvcc_stats().active_snapshots,0);
}

#[test]
fn conflicts_statement_abort_and_callback_unwind_do_not_leak_admission_slots() {
    let (_dir,root)=fixture::migrated(); let db=HistorySqlEngine::open_with_resource_limits(&root,HistoryLimits::default(),TransactionLimits { max_active:2 }).unwrap();
    db.with_session(|a|{
        a.execute("BEGIN")?; a.execute("UPDATE bank.public.accounts SET balance=111 WHERE id=1")?;
        db.with_session(|b|{ b.execute("BEGIN")?; b.execute("UPDATE bank.public.accounts SET balance=222 WHERE id=1")?; b.execute("COMMIT")?; Ok(()) })?;
        assert!(matches!(a.execute("COMMIT"),Err(SqlError::Domain(DomainError::Conflict)))); assert!(!a.in_transaction());
        a.execute("BEGIN")?; assert!(a.execute("SELECT missing FROM bank.public.accounts").is_err()); assert!(!a.in_transaction()); Ok(())
    }).unwrap(); assert_eq!(db.mvcc_stats().active_snapshots,0);
    let unwind=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{
        let _:Result<(),SqlError>=db.with_session(|s|{ s.execute("BEGIN")?; panic!("test callback unwind") });
    })); assert!(unwind.is_err()); assert_eq!(db.mvcc_stats().active_snapshots,0);
    assert_eq!(fixture::query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(222)]]);
}
