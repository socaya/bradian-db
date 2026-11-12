//! G16 native SQL execution across multiple tables in one shared WAL.
//!
//! DDL is created with the existing G12 engine, then SQL domain registration
//! is explicit. Existing legacy table stores are never silently migrated.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn setup(root: &std::path::Path) {
    let ddl = SqlEngine::open(root).unwrap();
    for statement in [
        "CREATE DATABASE banking",
        "CREATE SCHEMA banking.public",
        "CREATE TABLE banking.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE banking.public.movements (id BIGINT NOT NULL, account_id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
    ] {
        assert_eq!(ddl.execute(statement).unwrap(), SqlResultSet::Command { affected: 0 });
    }
    // Drop the first SQL engine to release the catalog's exclusive RowStore.
}

fn open_registered(root: &std::path::Path) -> DomainSqlEngine {
    let db = DomainSqlEngine::open(root).unwrap();
    db.register_catalog_table("banking.public.accounts").unwrap();
    db.register_catalog_table("banking.public.movements").unwrap();
    db
}

#[test]
fn sql_two_table_commit_is_one_wal_record_and_survives_restart() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let lsn;
    {
        let db = open_registered(dir.path());
        let start = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN TRANSACTION")?;
            s.execute("INSERT INTO banking.public.accounts (id,balance) VALUES (10,800)")?;
            s.execute("INSERT INTO banking.public.movements (id,account_id,amount) VALUES (40,10,-200)")?;
            assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=10")?,
                SqlResultSet::Rows {
                    columns: vec!["balance".into()],
                    rows: vec![vec![Value::Int64(800)]],
                });
            assert_eq!(s.execute("SELECT amount FROM banking.public.movements WHERE id=40")?,
                SqlResultSet::Rows {
                    columns: vec!["amount".into()],
                    rows: vec![vec![Value::Int64(-200)]],
                });
            s.execute("COMMIT")?;
            assert_eq!(s.last_commit_lsn(), Some(start + 1));
            Ok(())
        }).unwrap();
        lsn = db.last_lsn();
        assert_eq!(lsn, start + 1);
        db.checkpoint().unwrap();
    }
    let reopened = DomainSqlEngine::open(dir.path()).unwrap();
    assert_eq!(reopened.last_lsn(), lsn);
    reopened.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(session.execute("SELECT id,balance FROM banking.public.accounts")?,
            SqlResultSet::Rows {
                columns: vec!["id".into(), "balance".into()],
                rows: vec![vec![Value::Int64(10), Value::Int64(800)]],
            });
        assert_eq!(session.execute("SELECT account_id,amount FROM banking.public.movements")?,
            SqlResultSet::Rows {
                columns: vec!["account_id".into(), "amount".into()],
                rows: vec![vec![Value::Int64(10), Value::Int64(-200)]],
            });
        session.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn rollback_and_session_drop_abort_both_tables() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    let initial = db.last_lsn();
    db.with_session(|session| {
        session.execute("BEGIN")?;
        session.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(1,1000)")?;
        session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(1,1,1000)")?;
        session.execute("ROLLBACK")?;
        session.execute("BEGIN")?;
        session.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(2,2000)")?;
        Ok(()) // open transaction dropped
    }).unwrap();
    assert_eq!(db.last_lsn(), initial);
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(session.execute("SELECT * FROM banking.public.accounts")?,
            SqlResultSet::Rows {
                columns: vec!["id".into(), "balance".into()],
                rows: vec![],
            });
        assert_eq!(session.execute("SELECT * FROM banking.public.movements")?,
            SqlResultSet::Rows {
                columns: vec!["id".into(), "account_id".into(), "amount".into()],
                rows: vec![],
            });
        Ok(())
    }).unwrap();
}

#[test]
fn statement_error_discards_all_prior_changes_across_tables() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    db.with_session(|session| {
        session.execute("BEGIN")?;
        session.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(1,500)")?;
        assert!(session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(1,1,NULL)").is_err());
        assert!(!session.in_transaction());
        assert!(session.execute("COMMIT").is_err());
        session.execute("BEGIN")?;
        session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(2,1,20)")?;
        session.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert!(matches!(session.execute("SELECT id FROM banking.public.accounts")?,
            SqlResultSet::Rows { rows, .. } if rows.is_empty()));
        assert_eq!(session.execute("SELECT id FROM banking.public.movements")?,
            SqlResultSet::Rows {
                columns: vec!["id".into()],
                rows: vec![vec![Value::Int64(2)]],
            });
        Ok(())
    }).unwrap();
}

#[test]
fn sql_cross_table_update_and_delete_are_one_atomic_commit() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    db.with_session(|session| {
        session.execute("BEGIN")?;
        session.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(3,1000)")?;
        session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(7,3,100)")?;
        session.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let old_lsn = db.last_lsn();
    db.with_session(|session| {
        session.execute("BEGIN")?;
        session.execute("UPDATE banking.public.accounts SET balance=900 WHERE id=3")?;
        session.execute("DELETE FROM banking.public.movements WHERE id=7")?;
        session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(8,3,-100)")?;
        session.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), old_lsn + 1);
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(session.execute("SELECT balance FROM banking.public.accounts WHERE id=3")?,
            SqlResultSet::Rows {
                columns: vec!["balance".into()],
                rows: vec![vec![Value::Int64(900)]],
            });
        assert_eq!(session.execute("SELECT id FROM banking.public.movements")?,
            SqlResultSet::Rows {
                columns: vec!["id".into()],
                rows: vec![vec![Value::Int64(8)]],
            });
        Ok(())
    }).unwrap();
}

#[test]
fn opening_root_with_legacy_table_files_refuses_unsafe_dual_storage() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    // Standard G12 SQL DML provisions a separate per-table rows.wal.
    {
        let legacy = SqlEngine::open(dir.path()).unwrap();
        legacy.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(2,200)").unwrap();
    }
    assert!(matches!(DomainSqlEngine::open(dir.path()), Err(SqlError::Unsupported(_))));
}

#[test]
fn registration_of_nonexistent_table_is_rejected_without_creating_a_pin() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = DomainSqlEngine::open(dir.path()).unwrap();
    let initial = db.last_lsn();
    assert!(matches!(db.register_catalog_table("banking.public.unknown"), Err(SqlError::Resolution(_))));
    assert_eq!(db.last_lsn(), initial);
}

#[test]
fn transaction_sessions_reject_nested_begin_ddl_and_unregistered_tables() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = DomainSqlEngine::open(dir.path()).unwrap();
    db.register_catalog_table("banking.public.accounts").unwrap();
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert!(session.execute("BEGIN").is_err());
        assert!(!session.in_transaction());
        session.execute("BEGIN")?;
        assert!(session.execute("CREATE DATABASE unknown").is_err());
        assert!(!session.in_transaction());
        session.execute("BEGIN")?;
        assert!(session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(1,1,1)").is_err());
        assert!(!session.in_transaction());
        Ok(())
    }).unwrap();
}

#[test]
fn sql_session_rejects_multi_statement_and_unqualified_references() {
    let dir = tempdir().unwrap();
    setup(dir.path());
    let db = open_registered(dir.path());
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert!(session.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(1,1); COMMIT").is_err());
        assert!(!session.in_transaction());
        session.execute("BEGIN")?;
        assert!(session.execute("SELECT * FROM accounts").is_err());
        assert!(!session.in_transaction());
        Ok(())
    }).unwrap();
}
