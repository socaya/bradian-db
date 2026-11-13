#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated,modify,query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::runtime_limits::{ResourceError,ScanControl,ScanLimits};
use bradian_relational::transaction_domain::DomainError;
use bradian_relational::sql::{SqlError,SqlResultSet};
use bradian_relational::typed_table::Value;

fn control(rows: usize, bytes: usize, work: usize) -> ScanControl { ScanControl::new(ScanLimits { max_rows:rows,max_bytes:bytes,max_work:work }) }

#[test]
fn typed_and_historical_scans_are_scoped_to_the_selected_table() {
    let (_dir,root)=migrated(); let db=HistorySqlEngine::open(&root).unwrap();
    let horizon=db.last_lsn();
    db.with_session_control(control(1,10000,2),|s| {
        s.execute("BEGIN")?;
        for sql in ["SELECT id FROM bank.public.audit".to_string(),format!("SELECT id FROM bank.public.audit AS OF {horizon}")] {
            assert!(matches!(s.execute(&sql)?,SqlResultSet::Rows { rows,.. } if rows==vec![vec![Value::Int64(9)]]));
        }
        assert!(s.execute("SELECT id FROM bank.public.accounts").is_err());
        assert!(!s.in_transaction()); Ok(())
    }).unwrap();
    assert_eq!(db.mvcc_stats().active_snapshots,0);
}

#[test]
fn scan_failure_aborts_staged_work_or_requires_explicit_savepoint_recovery() {
    let (_dir,root)=migrated(); let db=HistorySqlEngine::open(&root).unwrap(); let before=db.last_lsn();
    db.with_session_control(control(1,10000,100),|s| {
        s.execute("BEGIN")?; s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        assert!(s.execute("SELECT id FROM bank.public.accounts").is_err()); assert!(s.execute("COMMIT").is_err()); Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(),before);
    db.with_session_control(control(1,10000,100),|s| {
        s.execute("BEGIN")?; s.execute("UPDATE bank.public.audit SET balance=91 WHERE id=9")?; s.execute("SAVEPOINT safe")?;
        assert!(s.execute("SELECT id FROM bank.public.accounts ORDER BY id LIMIT 1").is_err());
        assert!(s.execute("COMMIT").is_err()); s.execute("ROLLBACK TO safe")?;
        s.execute("SELECT id FROM bank.public.accounts WHERE id=1")?; s.execute("COMMIT")?; Ok(())
    }).unwrap();
    assert_eq!(query(&db,"SELECT balance FROM bank.public.audit WHERE id=9"),vec![vec![Value::Int64(91)]]);
}

#[test]
fn decoded_default_projection_is_budgeted_separately_from_encoded_input() {
    let (_dir,root)=migrated(); let mut db=HistorySqlEngine::open(&root).unwrap(); let old=db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl(&format!("ALTER TABLE bank.public.accounts ADD note TEXT DEFAULT '{}'","x".repeat(300))).unwrap();
    db.with_session_control(control(2,300,100),|s| {
        s.execute("BEGIN")?;
        assert!(matches!(s.execute(&format!("SELECT * FROM bank.public.accounts AS OF {old}"))?,SqlResultSet::Rows { rows,.. } if rows.len()==2));
        assert!(matches!(s.execute("SELECT * FROM bank.public.accounts"),Err(SqlError::Domain(DomainError::Resource(ResourceError::Limit { resource:"bytes",.. })))));
        assert!(!s.in_transaction()); Ok(())
    }).unwrap();
    assert_eq!(query(&db,"SELECT note FROM bank.public.accounts WHERE id=1"),vec![vec![Value::String("x".repeat(300))]]);
}

#[test]
fn counters_reset_per_query_but_cancellation_survives_rollback_and_allows_full_rollback() {
    let (_dir,root)=migrated(); let db=HistorySqlEngine::open(&root).unwrap(); let before=db.last_lsn();
    let cancellation=control(2,10000,4); let handle=cancellation.clone();
    db.with_session_control(cancellation,|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SELECT id FROM bank.public.accounts")?; s.execute("SELECT id FROM bank.public.accounts")?;
        s.execute("SAVEPOINT safe")?; s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        std::thread::spawn(move ||handle.cancel()).join().unwrap();
        assert!(matches!(s.execute("SELECT id FROM bank.public.accounts WHERE id=1"),Err(SqlError::Resource(ResourceError::Cancelled))));
        assert!(s.execute("COMMIT").is_err());
        s.execute("ROLLBACK TO safe")?;
        assert!(s.execute("SELECT id FROM bank.public.audit").is_err());
        s.execute("ROLLBACK")?; assert!(!s.in_transaction()); Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(),before); assert_eq!(db.mvcc_stats().active_snapshots,0);
    assert_eq!(query(&db,"SELECT balance FROM bank.public.audit WHERE id=9"),vec![vec![Value::Int64(90)]]);
}

#[test]
fn budgeted_scan_still_detects_concurrent_write_after_savepoint_recovery() {
    let (_dir,root)=migrated(); let db=HistorySqlEngine::open(&root).unwrap();
    db.with_session_control(control(1,10000,100),|s| {
        s.execute("BEGIN WITH READ VALIDATION")?; s.execute("SAVEPOINT safe")?;
        assert!(s.execute("SELECT id FROM bank.public.accounts LIMIT 0").is_err()); s.execute("ROLLBACK TO safe")?;
        modify(&db,&["INSERT INTO bank.public.messages(id,body) VALUES(1,'outside')"]);
        assert!(s.execute("COMMIT").is_err()); assert!(!s.in_transaction()); Ok(())
    }).unwrap();
    assert_eq!(db.mvcc_stats().active_snapshots,0);
}

#[test]
fn historical_projection_amplification_is_rejected_before_copy_and_staged_work_is_aborted() {
    let (_dir,root)=migrated(); let mut db=HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl(&format!("ALTER TABLE bank.public.accounts ADD note TEXT DEFAULT '{}'","x".repeat(1000))).unwrap();
    let columns=vec!["note";100].join(",");
    db.with_session_control(control(2,8000,10000),|s| {
        s.execute("BEGIN WITH READ VALIDATION")?; s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        assert!(matches!(s.execute(&format!("SELECT {columns} FROM bank.public.accounts WHERE id=1")),Err(SqlError::Resource(ResourceError::Limit { resource:"bytes",.. }))));
        assert!(!s.in_transaction()); assert!(s.execute("COMMIT").is_err()); Ok(())
    }).unwrap();
    assert_eq!(query(&db,"SELECT balance FROM bank.public.audit WHERE id=9"),vec![vec![Value::Int64(90)]]);
    assert_eq!(db.mvcc_stats().active_snapshots,0);
}
