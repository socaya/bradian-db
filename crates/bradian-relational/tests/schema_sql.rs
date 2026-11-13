//! G36: real exclusive ALTER, immutable epochs and historical column binding.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::{Column, DataType};
use bradian_relational::durable_mvcc::DurableMvccStore;
use bradian_relational::epoch_row::EpochSchemas;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::sql::SqlResultSet;
use bradian_relational::typed_table::Value;

#[test]
fn alter_requires_explicit_epoch_mode_and_projects_untouched_legacy_rows() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let original = db.last_lsn();
    assert!(db.execute_ddl("ALTER TABLE bank.public.accounts ADD COLUMN memo TEXT").is_err());
    assert_eq!(db.last_lsn(), original);
    db.enable_epoch_row_writes().unwrap();
    let before = db.last_lsn();
    let receipt = db.execute_ddl("ALTER TABLE bank.public.accounts ADD COLUMN memo TEXT NULL;").unwrap();
    assert_eq!(receipt.intent_lsn, before + 1);
    assert_eq!(receipt.schema_epoch, before + 2);
    assert_eq!(db.last_lsn(), receipt.schema_epoch);
    assert_eq!(query(&db, "SELECT * FROM bank.public.accounts"), vec![
        vec![Value::Int64(1), Value::Int64(100), Value::Null],
        vec![Value::Int64(2), Value::Int64(200), Value::Null],
    ]);
    assert_eq!(query(&db, &format!("SELECT * FROM bank.public.accounts AS OF {original}")), vec![
        vec![Value::Int64(1), Value::Int64(100)], vec![Value::Int64(2), Value::Int64(200)],
    ]);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute(&format!("SELECT memo FROM bank.public.accounts AS OF {original}")).is_err());
        assert!(!s.in_transaction());
        Ok(())
    }).unwrap();
    assert_eq!(db.schema_as_of("bank.public.accounts", original).unwrap().0.columns.len(), 2);
    assert_eq!(db.schema_as_of("bank.public.accounts", receipt.schema_epoch).unwrap().0.columns.len(), 3);
    db.checkpoint().unwrap();
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT memo FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null]]);
    assert_eq!(db.last_lsn(), receipt.schema_epoch);
}

#[test]
fn successive_revisions_decode_mixed_epochs_and_support_normal_dml() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let old = db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    let g35 = modify(&db, &["UPDATE bank.public.accounts SET balance=101 WHERE id=1"]);
    let first = db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo VARCHAR(80)").unwrap();
    let written = modify(&db, &[
        "UPDATE bank.public.accounts SET memo='reviewed' WHERE id=1",
        "INSERT INTO bank.public.accounts(id,balance,memo) VALUES(3,300,'new')",
    ]);
    let second = db.execute_ddl("ALTER TABLE bank.public.accounts ADD COLUMN score INTEGER").unwrap();
    let last = modify(&db, &[
        "UPDATE bank.public.accounts SET score=7 WHERE id=1",
        "UPDATE bank.public.accounts SET memo='legacy' WHERE id=2",
        "DELETE FROM bank.public.accounts WHERE id=3",
    ]);
    assert_eq!(query(&db, "SELECT * FROM bank.public.accounts WHERE id=1"), vec![vec![
        Value::Int64(1), Value::Int64(101), Value::String("reviewed".into()), Value::Int32(7),
    ]]);
    assert_eq!(query(&db, "SELECT score FROM bank.public.accounts WHERE id=2"), vec![vec![Value::Null]]);
    assert_eq!(query(&db, &format!("SELECT * FROM bank.public.accounts AS OF {g35} WHERE id=1")),
        vec![vec![Value::Int64(1), Value::Int64(101)]]);
    assert_eq!(query(&db, &format!("SELECT * FROM bank.public.accounts AS OF {written} WHERE id=1")),
        vec![vec![Value::Int64(1), Value::Int64(101), Value::String("reviewed".into())]]);
    assert_eq!(query(&db, &format!("SELECT score FROM bank.public.accounts AS OF {} WHERE id=1", second.schema_epoch)), vec![vec![Value::Null]]);
    assert_eq!(db.schema_epochs().iter().filter(|(id, _)| *id == first.table_id).count(), 3);
    let schemas = [old, first.schema_epoch, second.schema_epoch].map(|lsn|
        db.schema_as_of("bank.public.accounts", lsn).unwrap());
    db.checkpoint().unwrap();
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(db.last_lsn(), last);
    assert_eq!(query(&db, "SELECT memo FROM bank.public.accounts"), vec![
        vec![Value::String("reviewed".into())], vec![Value::String("legacy".into())],
    ]);
    assert_eq!(query(&db, &format!("SELECT balance FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![vec![Value::Int64(100)]]);
    drop(db);
    let mut registry = EpochSchemas::default();
    for (table, epoch) in schemas { registry.register(epoch, table).unwrap(); }
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    let values = raw.scan_as_of(last).unwrap();
    assert_eq!(values.iter().filter(|(k, _)| k.starts_with(b"BRPK")).count(), 3);
    assert!(values.iter().filter(|(k, _)| k.starts_with(b"BRPK")).any(|(_, bytes)|
        registry.decode(bytes).is_ok_and(|row| row.epoch == second.schema_epoch)));
}

#[test]
fn invalid_or_unsupported_ddl_never_advances_either_wal_or_poison_reads() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let before = db.last_lsn();
    for sql in [
        "ALTER TABLE bank.public.accounts ADD required TEXT NOT NULL",
        "ALTER TABLE bank.public.accounts ADD memo TEXT DEFAULT 'x'",
        "ALTER TABLE bank.public.accounts ADD id BIGINT",
        "ALTER TABLE bank.public.accounts DROP balance",
        "ALTER TABLE bank.public.accounts RENAME balance TO money",
        "ALTER TABLE bank.public.accounts ADD memo VARCHAR(0)",
        "ALTER TABLE bank.public.accounts ADD memo NUMERIC(0,0)",
        "ALTER TABLE bank.public.missing ADD memo TEXT",
        "ALTER TABLE public.accounts ADD memo TEXT",
        "ALTER TABLE bank.public.accounts ADD memo TEXT; DELETE FROM bank.public.accounts WHERE id=1",
    ] {
        assert!(db.execute_ddl(sql).is_err(), "accepted {sql}");
        assert_eq!(db.last_lsn(), before);
        assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(100)]]);
    }
    assert!(db.add_nullable_column("bank.public.accounts", Column {
        name: "required".into(), data_type: DataType::Text, nullable: false,
    }).is_err());
    assert!(db.add_nullable_column("bank.public.accounts", Column {
        name: "MixedCase".into(), data_type: DataType::Text, nullable: true,
    }).is_err());
    drop(db);
    let catalog = PersistentCatalog::open(root.join("catalog")).unwrap();
    assert!(catalog.proposed_table_revisions().is_empty());
    assert_eq!(catalog.last_lsn(), 6); // marker, database, schema, three tables
}

#[test]
fn backup_restore_and_retention_preserve_the_complete_schema_chain() {
    let (dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let old = db.last_lsn();
    db.enable_epoch_row_writes().unwrap();
    let first = db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    modify(&db, &["UPDATE bank.public.accounts SET memo='a' WHERE id=1"]);
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD score BIGINT").unwrap();
    let floor = modify(&db, &["UPDATE bank.public.accounts SET score=42 WHERE id=1"]);
    db.backup_to(dir.path().join("archive")).unwrap();
    restore_history_database(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let restored = HistorySqlEngine::open(dir.path().join("restored")).unwrap();
    assert_eq!(query(&restored, "SELECT score FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(42)]]);
    assert_eq!(query(&restored, &format!("SELECT * FROM bank.public.accounts AS OF {old} WHERE id=1")), vec![vec![Value::Int64(1), Value::Int64(100)]]);
    assert_eq!(restored.schema_as_of("bank.public.accounts", first.schema_epoch).unwrap().0.columns.len(), 3);
    drop(db);
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    assert!(raw.prune_through(floor).unwrap() > 0);
    raw.checkpoint().unwrap();
    drop(raw);
    let mut db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(query(&db, "SELECT memo,score FROM bank.public.accounts WHERE id=1"), vec![vec![Value::String("a".into()), Value::Int64(42)]]);
    assert_eq!(db.schema_epochs().iter().filter(|(id, _)| *id == first.table_id).count(), 3);
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute(&format!("SELECT * FROM bank.public.accounts AS OF {old}")).is_err());
        Ok(())
    }).unwrap();
    db.execute_ddl("ALTER TABLE bank.public.audit ADD checked BOOLEAN").unwrap();
}

#[test]
fn legacy_proposals_and_new_activation_cannot_silently_cross_protocols() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let (mut proposed, _) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
    proposed.columns.push(Column { name: "memo".into(), data_type: DataType::Text, nullable: true });
    db.prepare_nullable_addition("bank.public.accounts", &proposed).unwrap();
    let before = db.last_lsn();
    assert!(db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").is_err());
    assert_eq!(db.last_lsn(), before);
    let (_other, clean) = migrated();
    let mut db = HistorySqlEngine::open(&clean).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    let (mut proposed, _) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
    proposed.columns.push(Column { name: "next".into(), data_type: DataType::Text, nullable: true });
    let before = db.last_lsn();
    assert!(db.prepare_nullable_addition("bank.public.accounts", &proposed).is_err());
    assert_eq!(db.last_lsn(), before);
}

#[test]
fn revision_chain_binds_catalog_horizon_and_rejects_out_of_band_ddl() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    db.execute_ddl("ALTER TABLE bank.public.audit ADD checked BOOLEAN").unwrap();
    assert_eq!(query(&db, "SELECT checked FROM bank.public.audit"), vec![vec![Value::Null]]);
    drop(db);
    let catalog = PersistentCatalog::open(root.join("catalog")).unwrap();
    catalog.create_database("independent").unwrap();
    drop(catalog);
    assert!(HistorySqlEngine::open(&root).is_err());
}

#[test]
fn previously_valid_row_epoch_cannot_be_reused_for_a_later_write() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let (table, old_epoch) = db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let before = modify(&db, &["UPDATE bank.public.accounts SET balance=101 WHERE id=1"]);
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    drop(db);
    let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
    let mut registry = EpochSchemas::default();
    registry.register(old_epoch, table).unwrap();
    let (key, bytes) = raw.scan_as_of(before).unwrap().into_iter().find(|(_, bytes)|
        registry.decode(bytes).is_ok()).unwrap();
    let mut tx = raw.begin();
    tx.replace(&key, &bytes).unwrap();
    tx.commit().unwrap();
    let mut tx = raw.begin();
    tx.delete(&key).unwrap();
    tx.commit().unwrap();
    drop(raw);
    assert!(HistorySqlEngine::open(&root).is_err());
}

#[test]
fn describe_and_select_column_names_use_the_historical_schema() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    let before = db.last_lsn();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    db.with_session(|s| {
        assert!(matches!(s.execute(&format!("DESCRIBE TABLE bank.public.accounts AS OF {before}"))?,
            SqlResultSet::Rows { rows, .. } if rows.len() == 2));
        s.execute("BEGIN")?;
        match s.execute(&format!("SELECT * FROM bank.public.accounts AS OF {before}"))? {
            SqlResultSet::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id", "balance"]);
                assert!(rows.iter().all(|row| row.len() == 2));
            },
            other => panic!("unexpected result: {other:?}"),
        }
        Ok(())
    }).unwrap();
}
