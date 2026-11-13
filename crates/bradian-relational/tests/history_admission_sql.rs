#[path="support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated,query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::history_backup::restore_history_database_with_limits;
use bradian_relational::history_migration::migrate_shared_to_history_with_limits;
use bradian_relational::runtime_limits::{HistoryLimits,ResourceError};
use bradian_relational::transaction_domain::DomainError;
use bradian_relational::sql::SqlError;
use bradian_relational::typed_table::Value;

#[test]
fn sql_commit_refusal_preserves_old_rows_history_and_backup_restore_policy() {
    let (dir,root)=migrated(); let db=HistorySqlEngine::open(&root).unwrap(); let initial=db.history_usage(); let old=db.last_lsn(); drop(db);
    let policy=HistoryLimits { max_keys:initial.keys+2,max_versions:initial.versions+1,max_bytes:initial.bytes+10000 };
    let db=HistorySqlEngine::open_with_history_limits(&root,policy).unwrap();
    fixture::modify(&db,&["UPDATE bank.public.accounts SET balance=101 WHERE id=1"]); let full=db.history_usage(); let lsn=db.last_lsn();
    db.with_session(|s| { s.execute("BEGIN")?; s.execute("UPDATE bank.public.accounts SET balance=999 WHERE id=1")?;
        assert!(matches!(s.execute("COMMIT"),Err(SqlError::Domain(DomainError::Resource(ResourceError::HistoryLimit { resource:"versions",.. })))));
        assert!(!s.in_transaction()); Ok(()) }).unwrap();
    assert_eq!(db.last_lsn(),lsn); assert_eq!(db.history_usage(),full); assert_eq!(db.mvcc_stats().active_snapshots,0);
    assert_eq!(query(&db,&format!("SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1")),vec![vec![Value::Int64(100)]]);
    let archive=dir.path().join("archive"); db.backup_to(&archive).unwrap(); drop(db);
    let refused=dir.path().join("refused");
    assert!(restore_history_database_with_limits(&archive,&refused,HistoryLimits { max_versions:initial.versions,..policy }).is_err()); assert!(!refused.exists());
    let restored=dir.path().join("restored"); restore_history_database_with_limits(&archive,&restored,policy).unwrap();
    let db=HistorySqlEngine::open_with_history_limits(&restored,policy).unwrap(); assert_eq!(db.history_usage(),full);
    assert_eq!(query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(101)]]);
}

#[test]
fn configurable_migration_refuses_over_policy_staging_and_never_changes_source() {
    let (dir,_root)=migrated(); let source=dir.path().join("source"); let refused=dir.path().join("refused");
    let low=HistoryLimits { max_versions:0,..HistoryLimits::default() };
    assert!(migrate_shared_to_history_with_limits(&source,&refused,low).is_err()); assert!(!refused.exists());
    let target=dir.path().join("second"); let policy=HistoryLimits { max_keys:100,max_versions:100,max_bytes:1000000 };
    migrate_shared_to_history_with_limits(&source,&target,policy).unwrap();
    let db=HistorySqlEngine::open_with_history_limits(&target,policy).unwrap();
    assert_eq!(query(&db,"SELECT balance FROM bank.public.accounts WHERE id=1"),vec![vec![Value::Int64(100)]]);
    assert_eq!(db.history_limits().max_versions,100); assert!(db.history_usage().versions<=100);
    let source=bradian_relational::domain_sql::DomainSqlEngine::open(&source).unwrap();
    source.with_session(|s| { s.execute("BEGIN")?;
        assert!(matches!(s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,bradian_relational::sql::SqlResultSet::Rows { rows,.. } if rows==vec![vec![Value::Int64(100)]]));
        s.execute("ROLLBACK")?; Ok(()) }).unwrap();
}
