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
        assert!(s.execute("SELECT * FROM bank.public.accounts AS OF 1").is_err());
        // One malformed historical statement aborts this transaction.
        assert!(!s.in_transaction());
        s.execute("BEGIN")?;
        assert!(s.execute(&format!(
            "SELECT id FROM bank.public.accounts AS OF {}", latest + 100
        )).is_err());
        Ok(())
    }).unwrap();
}


#[test]
fn historical_sql_obeys_durable_retention_floor_after_reopen() {
    use bradian_relational::durable_mvcc::DurableMvccStore;
    let (_dir, _source, history) = migrated();
    let old_commit;
    let new_commit;
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        old_commit = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=600 WHERE id=1")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
        new_commit = db.last_lsn();
    }
    {
        // Only the dedicated G22 owner may prune its versioned domain,
        // and all SQL owners must first release the exclusive RowStore lock.
        let durable = DurableMvccStore::open(history.join("history-domain")).unwrap();
        durable.prune_through(new_commit).unwrap();
    }
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {old_commit} WHERE id=1"
        )).is_err());
        assert!(!s.in_transaction());
        s.execute("BEGIN")?;
        assert_eq!(s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {new_commit} WHERE id=1"
        ))?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(600)]],
        });
        Ok(())
    }).unwrap();
}

#[test]
fn lifecycle_managed_source_requires_separate_verified_conversion() {
    use bradian_relational::lifecycle::CatalogLifecycle;
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    seed(&source);
    {
        let owner = CatalogLifecycle::open(&source).unwrap();
        assert_eq!(owner.reconcile().unwrap().len(), 2);
    }
    let destination = dir.path().join("refused");
    assert!(matches!(
        migrate_shared_to_history(&source, &destination),
        Err(HistoryMigrationError::Invalid(_))
    ));
    assert!(!destination.exists());
}

#[test]
fn historical_sql_rejects_malformed_lsn_values_and_abort_is_atomic() {
    let (_dir, _source, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let before = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.accounts SET balance=111 WHERE id=1")?;
        assert!(s.execute("SELECT balance FROM bank.public.accounts AS OF -1 WHERE id=1").is_err());
        assert!(!s.in_transaction());
        assert!(s.execute("COMMIT").is_err());
        s.execute("BEGIN")?;
        assert!(s.execute("SELECT balance FROM bank.public.accounts AS OF 1.5 WHERE id=1").is_err());
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
}

#[test]
fn opening_history_rejects_invalid_retained_older_row_version() {
    use bradian_relational::durable_mvcc::DurableMvccStore;
    use bradian_relational::persistent_catalog::PersistentCatalog;
    use bradian_relational::typed_table::encode_key;
    let (_dir, _source, history) = migrated();
    let table = {
        let catalog = PersistentCatalog::open(history.join("catalog")).unwrap();
        catalog.snapshot().tables().find(|t| t.name == "accounts").unwrap().clone()
    };
    let positions = vec![0];
    let key = encode_key(&table, &positions, &[Value::Int64(1)]).unwrap();
    {
        let store = DurableMvccStore::open(history.join("history-domain")).unwrap();
        let mut tx = store.begin();
        // The newer value is intentionally invalid for the pinned typed schema.
        // It is subsequently tombstoned, so latest-row validation alone misses it.
        tx.replace(&key, b"not-a-typed-row").unwrap();
        tx.commit().unwrap();
        let mut tx = store.begin();
        tx.delete(&key).unwrap();
        tx.commit().unwrap();
        store.checkpoint().unwrap();
    }
    assert!(HistorySqlEngine::open(&history).is_err());
}

#[test]
fn opening_history_rejects_tombstoned_unregistered_table_key() {
    use bradian_relational::durable_mvcc::DurableMvccStore;
    let (_dir, _source, history) = migrated();
    let mut key = b"BRPK".to_vec();
    key.push(1);
    key.extend_from_slice(&u64::MAX.to_le_bytes());
    key.push(1);
    {
        let store = DurableMvccStore::open(history.join("history-domain")).unwrap();
        let mut tx = store.begin();
        tx.insert(&key, b"unknown-table").unwrap();
        tx.commit().unwrap();
        let mut tx = store.begin();
        tx.delete(&key).unwrap();
        tx.commit().unwrap();
    }
    assert!(HistorySqlEngine::open(&history).is_err());
}

#[test]
fn g24_backup_restores_retained_history_and_preserves_source() {
    use bradian_relational::history_backup::restore_history_database;
    let (dir, _source, history) = migrated();
    let archive = dir.path().join("g24-archive");
    let restored = dir.path().join("g24-restored");
    let old;
    let current;
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        old = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("UPDATE bank.public.accounts SET balance=400 WHERE id=1")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
        current = db.last_lsn();
        let receipt = db.backup_to(&archive).unwrap();
        assert_eq!(receipt.table_ids.len(), 2);
        assert!(receipt.retained_versions >= 2);
    }
    let receipt = restore_history_database(&archive, &restored).unwrap();
    assert_eq!(receipt.history.last_lsn, current);
    assert!(archive.join("history-backup.json").exists());
    let db = HistorySqlEngine::open(&restored).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert_eq!(s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1"
        ))?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(1000)]],
        });
        assert_eq!(s.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {current} WHERE id=1"
        ))?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(400)]],
        });
        Ok(())
    }).unwrap();
    assert!(HistorySqlEngine::open(&history).is_ok());
}

#[test]
fn g24_restore_refuses_tampered_history_archive_without_publishing() {
    use bradian_relational::history_backup::restore_history_database;
    use std::io::Write;
    let (dir, _source, history) = migrated();
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        db.backup_to(&archive).unwrap();
    }
    let mut f = std::fs::OpenOptions::new().append(true)
        .open(archive.join("history-domain").join("rows.wal")).unwrap();
    f.write_all(b"corrupt archive").unwrap();
    assert!(restore_history_database(&archive, &restored).is_err());
    assert!(!restored.exists());
}

#[test]
fn g24_backup_and_restore_refuse_occupied_or_overlapping_destinations() {
    use bradian_relational::history_backup::restore_history_database;
    let (dir, _source, history) = migrated();
    let archive = dir.path().join("archive");
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        assert!(db.backup_to(&history).is_err());
        db.backup_to(&archive).unwrap();
        assert!(db.backup_to(&archive).is_err());
    }
    assert!(restore_history_database(&archive, &history).is_err());
    let stage = dir.path().join(".restore.bradian-g24-incomplete");
    std::fs::create_dir(&stage).unwrap();
    assert!(restore_history_database(&archive, dir.path().join("restore")).is_err());
    assert!(!dir.path().join("restore").exists());
}

#[test]
fn g25_verified_g18_lifecycle_cutover_preserves_original_and_rows() {
    use bradian_relational::lifecycle::CatalogLifecycle;
    use bradian_relational::lifecycle_history_migration::migrate_lifecycle_to_history;
    let dir = tempdir().unwrap();
    let source = dir.path().join("g18-source");
    let destination = dir.path().join("g25-history");
    seed(&source);
    let original_lsn;
    {
        let owner = CatalogLifecycle::open(&source).unwrap();
        assert_eq!(owner.reconcile().unwrap().len(), 2);
        original_lsn = owner.last_domain_lsn();
    }
    let result = migrate_lifecycle_to_history(&source, &destination).unwrap();
    assert_eq!(result.imported.rows, 3);
    assert_eq!(result.imported.tables, 2);
    assert_eq!(result.captured_domain_lsn, original_lsn);
    assert!(source.join("lifecycle").exists());
    assert!(source.join("shared-domain").exists());
    assert!(!destination.join("lifecycle").exists());
    assert!(!destination.join("shared-domain").exists());
    {
        let historical = HistorySqlEngine::open(&destination).unwrap();
        historical.with_session(|session| {
            session.execute("BEGIN")?;
            assert_eq!(
                session.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
                SqlResultSet::Rows {
                    columns: vec!["balance".into()],
                    rows: vec![vec![Value::Int64(1000)]],
                }
            );
            Ok(())
        }).unwrap();
    }
    // The original G18 owner still opens and retains independently usable data.
    let original = CatalogLifecycle::open(&source).unwrap();
    assert_eq!(original.reconcile().unwrap().len(), 2);
}

#[test]
fn g25_refuses_open_lifecycle_owner_and_previous_target() {
    use bradian_relational::lifecycle::CatalogLifecycle;
    use bradian_relational::lifecycle_history_migration::migrate_lifecycle_to_history;
    let dir = tempdir().unwrap();
    let source = dir.path().join("g18-source");
    let destination = dir.path().join("history");
    seed(&source);
    let owner = CatalogLifecycle::open(&source).unwrap();
    assert!(migrate_lifecycle_to_history(&source, &destination).is_err());
    assert!(!destination.exists());
    drop(owner);
    migrate_lifecycle_to_history(&source, &destination).unwrap();
    assert!(migrate_lifecycle_to_history(&source, &destination).is_err());
}

#[test]
fn g25_refuses_incomplete_migration_staging_and_wrong_source_format() {
    use bradian_relational::lifecycle::CatalogLifecycle;
    use bradian_relational::lifecycle_history_migration::migrate_lifecycle_to_history;
    let dir = tempdir().unwrap();
    let source = dir.path().join("source");
    let destination = dir.path().join("history");
    seed(&source);
    // Only G18 lifecycle source format is eligible.
    assert!(migrate_lifecycle_to_history(&source, &destination).is_err());
    {
        let owner = CatalogLifecycle::open(&source).unwrap();
        assert_eq!(owner.reconcile().unwrap().len(), 2);
    }
    let staging = dir.path().join(".history.bradian-g25-source-archive");
    std::fs::create_dir(&staging).unwrap();
    assert!(migrate_lifecycle_to_history(&source, &destination).is_err());
    assert!(!destination.exists());
    std::fs::remove_dir(&staging).unwrap();
    // The physical source tree may not contain the new target.
    assert!(migrate_lifecycle_to_history(&source, source.join("nested-history")).is_err());
}

#[test]
fn g26_describe_table_as_of_returns_persisted_schema_epoch() {
    let (_dir, _source, history) = migrated();
    let first;
    let expected;
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        first = db.last_lsn();
        let epochs = db.schema_epochs();
        assert_eq!(epochs.len(), 2);
        let (schema, registered) = db.schema_as_of("bank.public.accounts", first).unwrap();
        assert_eq!(schema.name, "accounts");
        assert_eq!(registered, epochs.iter().find(|(id, _)| *id == schema.id).unwrap().1);
        expected = db.with_session(|session| {
            session.execute(&format!(
                "DESCRIBE TABLE bank.public.accounts AS OF {first}"
            ))
        }).unwrap();
        match &expected {
            SqlResultSet::Rows { columns, rows } => {
                assert_eq!(columns, &vec![
                    "column_name".to_string(), "data_type".to_string(),
                    "nullable".to_string(), "schema_registered_lsn".to_string(),
                ]);
                assert_eq!(rows.len(), 2);
                assert_eq!(rows[0][0], Value::String("id".into()));
                assert_eq!(rows[1][0], Value::String("balance".into()));
                assert_eq!(rows[0][3], Value::String(registered.to_string()));
            }
            _ => panic!("DESCRIBE must return schema rows"),
        }
        db.checkpoint().unwrap();
    }
    let db = HistorySqlEngine::open(&history).unwrap();
    assert_eq!(db.with_session(|session| {
        session.execute(&format!(
            "DESCRIBE TABLE bank.public.accounts AS OF {first}"
        ))
    }).unwrap(), expected);
}

#[test]
fn g26_schema_history_rejects_pre_registration_and_invalid_horizons() {
    let (_dir, _source, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let latest = db.last_lsn();
    let registration = db.schema_epochs().iter().map(|(_, lsn)| *lsn).max().unwrap();
    assert!(registration > 1);
    let earlier = registration - 1;
    assert!(db.schema_as_of("bank.public.movements", 0).is_err());
    assert!(db.schema_as_of("bank.public.accounts", latest + 1).is_err());
    db.with_session(|session| {
        assert!(session.execute(
            "DESCRIBE TABLE bank.public.accounts AS OF -1"
        ).is_err());
        assert!(session.execute(&format!(
            "DESCRIBE TABLE bank.public.movements AS OF {}",
            latest + 1
        )).is_err());
        // A failed read-only inspection must not publish a WAL write.
        assert!(!session.in_transaction());
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), latest);
    // At least one G23 table is registered at the final schema-batch LSN.
    assert!(db.schema_as_of("bank.public.accounts", earlier).is_err());
    assert!(db.schema_as_of("bank.public.movements", earlier).is_err());
}

#[test]
fn g27_preview_projects_retained_rows_without_changing_the_history_wal() {
    use bradian_relational::{Column, DataType};
    let (_dir, _source, history) = migrated();
    let before;
    let old_rows;
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        before = db.last_lsn();
        let (original, _) = db.schema_as_of("bank.public.accounts", before).unwrap();
        let mut proposed = original.clone();
        proposed.columns.push(Column {
            name: "comment".into(), data_type: DataType::Text, nullable: true,
        });
        old_rows = db.preview_nullable_additions_as_of(
            "bank.public.accounts", before, &proposed,
        ).unwrap();
        assert_eq!(old_rows.len(), 2);
        assert_eq!(old_rows[0], vec![
            Value::Int64(1), Value::Int64(1000), Value::Null,
        ]);
        assert_eq!(old_rows[1], vec![
            Value::Int64(2), Value::Int64(600), Value::Null,
        ]);
        assert_eq!(db.last_lsn(), before);
        db.checkpoint().unwrap();
    }
    let db = HistorySqlEngine::open(&history).unwrap();
    let (original, _) = db.schema_as_of("bank.public.accounts", before).unwrap();
    let mut proposed = original.clone();
    proposed.columns.push(Column {
        name: "comment".into(), data_type: DataType::Text, nullable: true,
    });
    assert_eq!(db.preview_nullable_additions_as_of(
        "bank.public.accounts", before, &proposed,
    ).unwrap(), old_rows);
    assert_eq!(db.last_lsn(), before);
    // Current catalog remains the unmodified original schema.
    assert_eq!(db.schema_as_of("bank.public.accounts", before).unwrap().0.columns.len(), 2);
    proposed.columns.last_mut().unwrap().nullable = false;
    assert!(db.preview_nullable_additions_as_of(
        "bank.public.accounts", before, &proposed,
    ).is_err());
}

#[test]
fn g28_prepared_schema_intent_survives_restart_without_activating_ddl() {
    use bradian_relational::{Column, DataType};
    let (_dir, _source, history) = migrated();
    let prepared_lsn;
    let old_lsn;
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        old_lsn = db.last_lsn();
        let (old, _) = db.schema_as_of("bank.public.accounts", old_lsn).unwrap();
        let mut proposed = old.clone();
        proposed.columns.push(Column {
            name: "memo".into(), data_type: DataType::Text, nullable: true,
        });
        prepared_lsn = db.prepare_nullable_addition("bank.public.accounts", &proposed).unwrap();
        assert!(prepared_lsn > old_lsn);
        assert!(db.prepare_nullable_addition("bank.public.accounts", &proposed).is_err());
        db.checkpoint().unwrap();
    }
    let db = HistorySqlEngine::open(&history).unwrap();
    let intents = db.prepared_schema_proposals();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].2, prepared_lsn);
    assert_eq!(intents[0].1.columns.len(), 3);
    assert_eq!(db.schema_as_of("bank.public.accounts", prepared_lsn).unwrap().0.columns.len(), 2);
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(session.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows {
                columns: vec!["balance".into()],
                rows: vec![vec![Value::Int64(1000)]],
            });
        assert_eq!(session.execute(&format!(
            "SELECT balance FROM bank.public.accounts AS OF {old_lsn} WHERE id=1"
        ))?, SqlResultSet::Rows {
            columns: vec!["balance".into()],
            rows: vec![vec![Value::Int64(1000)]],
        });
        Ok(())
    }).unwrap();
}

#[test]
fn g28_schema_proposal_rejects_incompatible_changes_without_wal_write() {
    use bradian_relational::{Column, DataType};
    let (_dir, _source, history) = migrated();
    let db = HistorySqlEngine::open(&history).unwrap();
    let before = db.last_lsn();
    let (old, _) = db.schema_as_of("bank.public.accounts", before).unwrap();
    let mut incompatible = old.clone();
    incompatible.columns.push(Column {
        name: "required".into(), data_type: DataType::Text, nullable: false,
    });
    assert!(db.prepare_nullable_addition("bank.public.accounts", &incompatible).is_err());
    incompatible = old.clone();
    incompatible.columns[1].name = "renamed".into();
    incompatible.columns.push(Column {
        name: "extra".into(), data_type: DataType::Text, nullable: true,
    });
    assert!(db.prepare_nullable_addition("bank.public.accounts", &incompatible).is_err());
    assert_eq!(db.last_lsn(), before);
}

#[test]
fn g28_prepared_intents_survive_g24_history_backup_restore() {
    use bradian_relational::{Column, DataType};
    use bradian_relational::history_backup::restore_history_database;
    let (dir, _source, history) = migrated();
    let archive = dir.path().join("archive");
    let recovered = dir.path().join("restored");
    {
        let db = HistorySqlEngine::open(&history).unwrap();
        let (old, _) = db.schema_as_of("bank.public.movements", db.last_lsn()).unwrap();
        let mut next = old.clone();
        next.columns.push(Column {
            name: "note".into(), data_type: DataType::Text, nullable: true,
        });
        db.prepare_nullable_addition("bank.public.movements", &next).unwrap();
        db.backup_to(&archive).unwrap();
    }
    restore_history_database(&archive, &recovered).unwrap();
    let db = HistorySqlEngine::open(&recovered).unwrap();
    let intents = db.prepared_schema_proposals();
    assert_eq!(intents.len(), 1);
    assert_eq!(intents[0].1.columns.last().unwrap().name, "note");
    assert_eq!(db.schema_as_of("bank.public.movements", db.last_lsn()).unwrap().0.columns.len(), 3 - 1);
}
