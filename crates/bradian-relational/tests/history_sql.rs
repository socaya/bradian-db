//! G23: explicit G16 -> G22 durable typed SQL history cutover and AS OF.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::{migrate_shared_to_history, HistoryMigrationError};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(root: &std::path::Path) {
    {
        let ddl = SqlEngine::open(root).unwrap();
        for sql in [
            "CREATE DATABASE bank",
            "CREATE SCHEMA bank.public",
            "CREATE TABLE bank.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE bank.public.movements (id BIGINT NOT NULL, account_id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let db = DomainSqlEngine::open(root).unwrap();
        db.register_catalog_table("bank.public.accounts").unwrap();
        db.register_catalog_table("bank.public.movements").unwrap();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(1,1000)")?;
            s.execute("INSERT INTO bank.public.accounts(id,balance) VALUES(2,600)")?;
            s.execute("INSERT INTO bank.public.movements(id,account_id,amount) VALUES(8,1,100)")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
}

fn migrated() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("legacy-shared");
    let history = dir.path().join("durable-history");
    seed(&source);
    let report = migrate_shared_to_history(&source, &history).unwrap();
    assert_eq!(report.tables, 2);
    assert_eq!(report.rows, 3);
    (dir, source, history)
}

#[test]
fn verified_cutover_keeps_original_and_queries_typed_rows() {
    let (_dir, source, history) = migrated();
    assert!(source.join("shared-domain").exists());
    assert!(!history.join("shared-domain").exists());
    assert!(history.join("history-domain").exists());
    let db = HistorySqlEngine::open(&history).unwrap();
    assert_eq!(db.registered_table_ids().len(), 2);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows {
                columns: vec!["balance".into()],
                rows: vec![vec![Value::Int64(1000)]],
            });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    drop(db);
    let old = DomainSqlEngine::open(&source).unwrap();
    old.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT id FROM bank.public.movements")?,
            SqlResultSet::Rows {
                columns: vec!["id".into()],
                rows: vec![vec![Value::Int64(8)]],
            });
        Ok(())
    }).unwrap();
}

#[test]
fn historical_select_as_of_persists_across_restart() {
    let (_dir, _source, history) = migrated();
    let before;
    let after;
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        before = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=750 WHERE id=1")?;
            s.execute("INSERT INTO bank.public.movements(id,account_id,amount) VALUES(9,1,-250)")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
        after = db.last_lsn();
        assert_eq!(after, before + 1);
        db.checkpoint().unwrap();
    }
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let old = format!("SELECT balance FROM bank.public.accounts AS OF {before} WHERE id=1");
        assert_eq!(s.execute(&old)?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(1000)]],
        });
        let newer = format!("SELECT balance FROM bank.public.accounts AS OF {after} WHERE id=1");
        assert_eq!(s.execute(&newer)?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(750)]],
        });
        let old_movements = format!("SELECT id FROM bank.public.movements AS OF {before}");
        assert_eq!(s.execute(&old_movements)?, SqlResultSet::Rows {
            columns: vec!["id".into()],
            rows: vec![vec![Value::Int64(8)]],
        });
        let new_movements = format!("SELECT id FROM bank.public.movements AS OF {after}");
        assert_eq!(s.execute(&new_movements)?, SqlResultSet::Rows {
            columns: vec!["id".into()],
            rows: vec![vec![Value::Int64(8)], vec![Value::Int64(9)]],
        });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn deletion_history_and_rollback_behave_correctly() {
    let (_dir, _source, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let before = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM bank.public.accounts WHERE id=2")?;
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM bank.public.accounts WHERE id=2")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let after = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {before} WHERE id=2"
        ))?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(600)]],
        });
        assert_eq!(s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {after} WHERE id=2"
        ))?, SqlResultSet::Rows { columns: vec!["balance".into()], rows: vec![] });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn source_storage_locks_block_migration_without_publishing_target() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let dst = dir.path().join("dst");
    seed(&src);
    let active = DomainSqlEngine::open(&src).unwrap();
    assert!(migrate_shared_to_history(&src, &dst).is_err());
    assert!(!dst.exists());
    drop(active);
}

#[test]
fn repeated_cutover_and_overlapping_destination_fail_closed() {
    let (_dir, source, history) = migrated();
    assert!(migrate_shared_to_history(&source, &history).is_err());
    assert!(matches!(
        migrate_shared_to_history(&source, source.join("child")),
        Err(HistoryMigrationError::Invalid(_))
    ));
}

#[test]
fn incomplete_staging_and_orphan_legacy_files_fail_closed() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    seed(&src);
    let stage = dir.path().join(".next.bradian-g23-incomplete");
    std::fs::create_dir(&stage).unwrap();
    assert!(matches!(
        migrate_shared_to_history(&src, dir.path().join("next")),
        Err(HistoryMigrationError::Invalid(_))
    ));
    std::fs::remove_dir(&stage).unwrap();
    std::fs::create_dir_all(src.join("tables").join("999")).unwrap();
    assert!(matches!(
        migrate_shared_to_history(&src, dir.path().join("next")),
        Err(HistoryMigrationError::Invalid(_))
    ));
}

#[test]
fn mvcc_history_root_rejects_original_g16_ownership_and_format() {
    let (_dir, source, history) = migrated();
    assert!(HistorySqlEngine::open(&source).is_err());
    assert!(DomainSqlEngine::open(&history).is_err());
    let db = HistorySqlEngine::open(&history).unwrap();
    assert!(HistorySqlEngine::open(&history).is_err());
    drop(db);
    assert!(HistorySqlEngine::open(&history).is_ok());
}

#[test]
fn as_of_before_first_import_visibility_returns_no_rows() {
    let (_dir, _source, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let latest = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        // Durable MVCC namespace initialization LSN contains no typed rows.
        assert_eq!(s.execute("SELECT * FROM bank.public.accounts AS OF 1")?,
            SqlResultSet::Rows {
                columns: vec!["id".into(), "balance".into()],
                rows: vec![],
            });
        assert!(s.execute(&format!(
            "SELECT id FROM bank.public.accounts AS OF {}", latest + 100
        )).is_err());
        Ok(())
    }).unwrap();
}
