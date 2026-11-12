//! G17 offline cutover: legacy G12 row stores -> one shared-WAL domain.

use std::fs;

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::legacy_migration::{migrate_legacy_sql, MigrationError};
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn create_legacy(root: &std::path::Path) {
    let db = SqlEngine::open(root).unwrap();
    for sql in [
        "CREATE DATABASE banking",
        "CREATE SCHEMA banking.public",
        "CREATE TABLE banking.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE banking.public.movements (id BIGINT NOT NULL, account_id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE banking.public.empty (id BIGINT NOT NULL, note TEXT NULL, PRIMARY KEY(id))",
    ] {
        assert_eq!(db.execute(sql).unwrap(), SqlResultSet::Command { affected: 0 });
    }
    for sql in [
        "INSERT INTO banking.public.accounts (id,balance) VALUES (1,1000)",
        "INSERT INTO banking.public.accounts (id,balance) VALUES (2,500)",
        "INSERT INTO banking.public.movements (id,account_id,amount) VALUES (5,1,-100)",
    ] { db.execute(sql).unwrap(); }
    // Database and all legacy row-store file locks are released on drop.
}

fn select_domain(root: &std::path::Path) -> Vec<Vec<Value>> {
    let db = DomainSqlEngine::open(root).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        let rows = match s.execute("SELECT id,balance FROM banking.public.accounts")? {
            SqlResultSet::Rows { rows, .. } => rows,
            _ => panic!("expected rows"),
        };
        s.execute("ROLLBACK")?;
        Ok(rows)
    }).unwrap()
}

#[test]
fn migrates_multiple_legacy_tables_and_preserves_original_data() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    let dst = base.path().join("shared");
    create_legacy(&src);
    let report = migrate_legacy_sql(&src, &dst).unwrap();
    assert_eq!(report.tables.len(), 3);
    assert_eq!(report.total_rows, 3);
    assert!(report.catalog_lsn >= 6);
    assert!(report.shared_domain_lsn >= 3);
    assert_eq!(select_domain(&dst), vec![
        vec![Value::Int64(1),Value::Int64(1000)],
        vec![Value::Int64(2),Value::Int64(500)],
    ]);
    {
        let engine = DomainSqlEngine::open(&dst).unwrap();
        engine.with_session(|s| {
            s.execute("BEGIN")?;
            assert_eq!(s.execute("SELECT id,amount FROM banking.public.movements")?,
                SqlResultSet::Rows {
                    columns: vec!["id".into(), "amount".into()],
                    rows: vec![vec![Value::Int64(5), Value::Int64(-100)]],
                });
            assert_eq!(s.execute("SELECT id,note FROM banking.public.empty")?,
                SqlResultSet::Rows {
                    columns: vec!["id".into(),"note".into()],
                    rows: vec![],
                });
            s.execute("ROLLBACK")?;
            Ok(())
        }).unwrap();
    }
    let legacy = SqlEngine::open(&src).unwrap();
    assert_eq!(legacy.execute("SELECT id,balance FROM banking.public.accounts").unwrap(),
        SqlResultSet::Rows {
            columns: vec!["id".into(),"balance".into()],
            rows: vec![
                vec![Value::Int64(1),Value::Int64(1000)],
                vec![Value::Int64(2),Value::Int64(500)],
            ],
        });
}

#[test]
fn migrated_shared_domain_supports_new_atomic_cross_table_sql() {
    let base = tempdir().unwrap();
    let source = base.path().join("legacy");
    let dest = base.path().join("dest");
    create_legacy(&source);
    migrate_legacy_sql(&source, &dest).unwrap();
    {
        let sql = DomainSqlEngine::open(&dest).unwrap();
        sql.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE banking.public.accounts SET balance=900 WHERE id=1")?;
            s.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(6,1,-100)")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    let reopened = DomainSqlEngine::open(&dest).unwrap();
    reopened.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT balance FROM banking.public.accounts WHERE id=1")?,
            SqlResultSet::Rows {
                columns:vec!["balance".into()],
                rows:vec![vec![Value::Int64(900)]],
            });
        assert_eq!(s.execute("SELECT amount FROM banking.public.movements WHERE id=6")?,
            SqlResultSet::Rows {
                columns:vec!["amount".into()],
                rows:vec![vec![Value::Int64(-100)]],
            });
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn refuses_source_with_active_legacy_catalog_lock() {
    let base = tempdir().unwrap();
    let source = base.path().join("legacy");
    create_legacy(&source);
    let active = SqlEngine::open(&source).unwrap();
    let destination = base.path().join("blocked");
    assert!(migrate_legacy_sql(&source, &destination).is_err());
    assert!(!destination.exists());
    drop(active);
}

#[test]
fn refuses_active_legacy_table_lock_even_when_catalog_is_free() {
    use bradian_relational::{Catalog, TableSpec};
    use bradian_relational::persistent_catalog::PersistentCatalog;
    use bradian_relational::typed_table::TableStore;

    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    create_legacy(&src);
    // Retrieve exact persistent schema, then separately lock a legacy table.
    let table = {
        let cat = PersistentCatalog::open(src.join("catalog")).unwrap();
        cat.snapshot().tables().find(|t| t.name == "accounts").unwrap().clone()
    };
    let legacy = TableStore::open(src.join("tables").join(table.id.0.to_string()), table).unwrap();
    let dest = base.path().join("blocked");
    assert!(migrate_legacy_sql(&src, &dest).is_err());
    assert!(!dest.exists());
    drop(legacy);
}

#[test]
fn unexpected_orphan_legacy_table_directory_is_refused() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    create_legacy(&src);
    fs::create_dir_all(src.join("tables").join("999999")).unwrap();
    let dest = base.path().join("blocked");
    assert!(matches!(
        migrate_legacy_sql(&src,&dest), Err(MigrationError::Invalid(_))
    ));
    assert!(!dest.exists());
}

#[test]
fn refuses_existing_target_without_overwriting_anything() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    create_legacy(&src);
    let dst = base.path().join("shared");
    fs::create_dir(&dst).unwrap();
    fs::write(dst.join("sentinel"), b"keep").unwrap();
    assert!(matches!(
        migrate_legacy_sql(&src, &dst), Err(MigrationError::Invalid(_))
    ));
    assert_eq!(fs::read(dst.join("sentinel")).unwrap(), b"keep");
}

#[test]
fn repeated_cutover_refuses_destination_that_already_exists() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    let dst = base.path().join("shared");
    create_legacy(&src);
    migrate_legacy_sql(&src, &dst).unwrap();
    assert!(migrate_legacy_sql(&src, &dst).is_err());
    assert_eq!(select_domain(&dst).len(), 2);
}

#[test]
fn cannot_migrate_source_into_its_own_subdirectory() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    create_legacy(&src);
    assert!(matches!(
        migrate_legacy_sql(&src, src.join("nested")),
        Err(MigrationError::Invalid(_))
    ));
}

#[test]
fn refuses_unfinished_previous_migration_staging_directory() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    create_legacy(&src);
    fs::create_dir(base.path().join(".shared.bradian-g17-incomplete")).unwrap();
    let dst = base.path().join("shared");
    assert!(matches!(
        migrate_legacy_sql(&src, &dst), Err(MigrationError::Invalid(_))
    ));
    assert!(!dst.exists());
}

#[test]
fn migration_with_only_catalog_metadata_keeps_unprovisioned_tables_empty() {
    let base = tempdir().unwrap();
    let src = base.path().join("legacy");
    let dst = base.path().join("shared");
    {
        let db = SqlEngine::open(&src).unwrap();
        db.execute("CREATE DATABASE reporting").unwrap();
        db.execute("CREATE SCHEMA reporting.public").unwrap();
        db.execute("CREATE TABLE reporting.public.empty (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    }
    let report = migrate_legacy_sql(&src,&dst).unwrap();
    assert_eq!(report.total_rows,0);
    assert_eq!(report.tables.len(),1);
    let db = DomainSqlEngine::open(&dst).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute("SELECT * FROM reporting.public.empty")?,
            SqlResultSet::Rows {columns: vec!["id".into()],rows: vec![]});
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
