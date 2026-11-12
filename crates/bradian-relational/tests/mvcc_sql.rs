//! G21 opt-in SQL over the original shared-WAL row format.
//! Tests exercise real G11 metadata, G15 registrations and G20 MVCC visibility.

use std::sync::{Arc, Barrier};
use std::thread;

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::transaction_domain::DomainError;
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn setup(root: &std::path::Path) {
    {
        let ddl = SqlEngine::open(root).unwrap();
        for sql in [
            "CREATE DATABASE banking",
            "CREATE SCHEMA banking.public",
            "CREATE TABLE banking.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE banking.public.movements (id BIGINT NOT NULL, account_id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        ] {
            ddl.execute(sql).unwrap();
        }
    }
    {
        let domain = DomainSqlEngine::open(root).unwrap();
        domain.register_catalog_table("banking.public.accounts").unwrap();
        domain.register_catalog_table("banking.public.movements").unwrap();
    }
}

fn insert_seed(engine: &MvccSqlEngine) {
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(1,1000)")?;
        s.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(2,500)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_sql_accepts_g15_format_and_commits_across_two_tables() {
    let base = tempdir().unwrap();
    setup(base.path());
    let committed;
    {
        let engine = MvccSqlEngine::open(base.path()).unwrap();
        assert_eq!(engine.registered_table_ids().len(), 2);
        engine.with_session(|s| {
            s.execute("BEGIN TRANSACTION")?;
            assert!(s.snapshot_lsn().is_some());
            s.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(10,900)")?;
            s.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(5,10,-100)")?;
            assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=10")?,
                SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(900)]] });
            s.execute("COMMIT")?;
            assert!(s.last_commit_lsn().is_some());
            Ok(())
        }).unwrap();
        committed = engine.last_lsn();
        engine.checkpoint().unwrap();
    }
    {
        // The previous G16 reader can reopen the exact same physical format.
        let legacy_sql = DomainSqlEngine::open(base.path()).unwrap();
        assert_eq!(legacy_sql.last_lsn(), committed);
        legacy_sql.with_session(|s| {
            s.execute("BEGIN")?;
            assert_eq!(s.execute("SELECT amount FROM banking.public.movements WHERE id=5")?,
                SqlResultSet::Rows { columns: vec!["amount".into()], rows: vec![vec![Value::Int64(-100)]] });
            Ok(())
        }).unwrap();
    }
    let reopened = MvccSqlEngine::open(base.path()).unwrap();
    assert_eq!(reopened.last_lsn(), committed);
    assert_eq!(reopened.mvcc_stats().versions_retained, 4); // two pins and two data rows
}

#[test]
fn independent_sql_sessions_keep_original_snapshot_visible() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    insert_seed(&engine);
    engine.with_session(|reader| {
        reader.execute("BEGIN")?;
        let original = reader.snapshot_lsn().unwrap();
        engine.with_session(|writer| {
            writer.execute("BEGIN")?;
            writer.execute("UPDATE banking.public.accounts SET balance=800 WHERE id=1")?;
            writer.execute("COMMIT")?;
            Ok(())
        })?;
        assert_eq!(reader.snapshot_lsn(), Some(original));
        assert_eq!(reader.execute("SELECT balance FROM banking.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(1000)]] });
        reader.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(800)]] });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn concurrent_disjoint_sql_updates_succeed_from_same_snapshot() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = Arc::new(MvccSqlEngine::open(base.path()).unwrap());
    insert_seed(&engine);
    let barrier = Arc::new(Barrier::new(2));
    let jobs = (0..2).map(|idx| {
        let engine = Arc::clone(&engine);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            engine.with_session(|s| {
                s.execute("BEGIN")?;
                if idx == 0 {
                    s.execute("UPDATE banking.public.accounts SET balance=950 WHERE id=1")?;
                } else {
                    s.execute("UPDATE banking.public.accounts SET balance=450 WHERE id=2")?;
                }
                barrier.wait();
                s.execute("COMMIT")?;
                Ok(())
            })
        })
    }).collect::<Vec<_>>();
    for job in jobs {
        job.join().unwrap().unwrap();
    }
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(950)]] });
        assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=2")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(450)]] });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn same_key_sql_writers_conflict_without_lost_update() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    insert_seed(&engine);
    engine.with_session(|stale| {
        stale.execute("BEGIN")?;
        stale.execute("UPDATE banking.public.accounts SET balance=700 WHERE id=1")?;
        engine.with_session(|fresh| {
            fresh.execute("BEGIN")?;
            fresh.execute("UPDATE banking.public.accounts SET balance=850 WHERE id=1")?;
            fresh.execute("COMMIT")?;
            Ok(())
        })?;
        assert!(matches!(stale.execute("COMMIT"), Err(SqlError::Domain(DomainError::Conflict))));
        assert!(!stale.in_transaction());
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(850)]] });
        Ok(())
    }).unwrap();
}

#[test]
fn full_sql_scan_then_write_rejects_intervening_phantom() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    insert_seed(&engine);
    engine.with_session(|old| {
        old.execute("BEGIN")?;
        assert!(matches!(
            old.execute("SELECT id FROM banking.public.accounts")?,
            SqlResultSet::Rows { rows, .. } if rows.len() == 2
        ));
        old.execute("UPDATE banking.public.accounts SET balance=900 WHERE id=1")?;
        engine.with_session(|newer| {
            newer.execute("BEGIN")?;
            newer.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(2,1,-100)")?;
            newer.execute("COMMIT")?;
            Ok(())
        })?;
        assert!(matches!(old.execute("COMMIT"), Err(SqlError::Domain(DomainError::Conflict))));
        Ok(())
    }).unwrap();
}

#[test]
fn rollback_and_failed_statement_abort_both_tables_without_partial_commit() {
    let base = tempdir().unwrap();
    setup(base.path());
    let engine = MvccSqlEngine::open(base.path()).unwrap();
    let before = engine.last_lsn();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(9,90)")?;
        s.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(9,9,-10)")?;
        s.execute("ROLLBACK")?;
        s.execute("BEGIN")?;
        s.execute("INSERT INTO banking.public.accounts(id,balance) VALUES(9,90)")?;
        assert!(s.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(9,9,NULL)").is_err());
        assert!(!s.in_transaction());
        assert!(s.execute("COMMIT").is_err());
        Ok(())
    }).unwrap();
    assert_eq!(engine.last_lsn(), before);
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(matches!(s.execute("SELECT * FROM banking.public.accounts")?,
            SqlResultSet::Rows { rows, .. } if rows.is_empty()));
        Ok(())
    }).unwrap();
}

#[test]
fn physical_legacy_rows_and_unregistered_tables_are_refused() {
    let base = tempdir().unwrap();
    setup(base.path());
    {
        let engine = MvccSqlEngine::open(base.path()).unwrap();
        engine.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute("SELECT * FROM banking.public.unknown").is_err());
            assert!(!s.in_transaction());
            Ok(())
        }).unwrap();
    }
    std::fs::create_dir_all(base.path().join("tables").join("1")).unwrap();
    assert!(matches!(MvccSqlEngine::open(base.path()), Err(SqlError::Unsupported(_))));
}

#[test]
fn mvcc_sql_rejects_unprovisioned_new_root() {
    let base = tempdir().unwrap();
    assert!(matches!(MvccSqlEngine::open(base.path()), Err(SqlError::Unsupported(_))));
}

#[test]
fn historical_versions_not_claimed_after_reopen_and_pins_released() {
    let base = tempdir().unwrap();
    setup(base.path());
    {
        let engine = MvccSqlEngine::open(base.path()).unwrap();
        insert_seed(&engine);
        engine.with_session(|reader| {
            reader.execute("BEGIN")?;
            engine.with_session(|writer| {
                writer.execute("BEGIN")?;
                writer.execute("UPDATE banking.public.accounts SET balance=777 WHERE id=1")?;
                writer.execute("COMMIT")?;
                Ok(())
            })?;
            assert!(engine.mvcc_stats().versions_retained > 4);
            reader.execute("ROLLBACK")?;
            Ok(())
        }).unwrap();
        assert_eq!(engine.mvcc_stats().active_snapshots, 0);
        assert_eq!(engine.mvcc_stats().versions_retained, 4);
    }
    let recovered = MvccSqlEngine::open(base.path()).unwrap();
    assert_eq!(recovered.mvcc_stats().versions_retained, 4);
    recovered.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![vec![Value::Int64(777)]] });
        Ok(())
    }).unwrap();
}
