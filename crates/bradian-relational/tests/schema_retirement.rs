//! G37: explicit retirement preserves inactive intent provenance and permits G36.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::{Column, DataType};
use bradian_relational::durable_mvcc::DurableMvccStore;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::typed_table::Value;
const RETIREMENT: &[u8] = b"\0bradian-schema-legacy-retirement-v1";

fn prepare(db: &HistorySqlEngine, name: &str) {
    let (mut table, _) = db.schema_as_of(name, db.last_lsn()).unwrap();
    table.columns.push(Column { name: "old_proposal".into(), data_type: DataType::Text, nullable: true });
    db.prepare_nullable_addition(name, &table).unwrap();
}

#[test]
fn retirement_does_not_apply_proposals_and_unlocks_recoverable_alter() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    prepare(&db, "bank.public.accounts");
    db.coordinate_prepared_schema("bank.public.accounts").unwrap();
    db.certify_schema_readiness("bank.public.accounts").unwrap();
    prepare(&db, "bank.public.audit");
    db.abort_prepared_schema("bank.public.audit").unwrap();
    let before = db.last_lsn();
    assert!(db.execute_ddl("ALTER TABLE bank.public.accounts ADD note TEXT").is_err());
    let receipt = db.retire_legacy_schema_intents().unwrap();
    assert_eq!(receipt.history_lsn, before + 1);
    assert_eq!(receipt.retired_records, 5);
    assert_eq!(receipt.catalog_lsn, 6);
    assert!(db.legacy_schema_intents_retired());
    assert!(db.prepared_schema_proposals().is_empty());
    assert_eq!(query(&db, "SELECT * FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(1), Value::Int64(100)]]);
    assert!(db.retire_legacy_schema_intents().is_err());
    assert_eq!(db.last_lsn(), before + 1);
    drop(db);
    let mut db = HistorySqlEngine::open(&root).unwrap();
    assert_eq!(db.ddl_coordination_records().len(), 1);
    assert_eq!(db.ddl_readiness_records().len(), 1);
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD note TEXT").unwrap();
    assert_eq!(query(&db, "SELECT note FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null]]);
    modify(&db, &["UPDATE bank.public.accounts SET note='fresh' WHERE id=1"]);
    drop(db);
    let db = HistorySqlEngine::open(&root).unwrap();
    assert!(db.legacy_schema_intents_retired());
    assert_eq!(query(&db, "SELECT note FROM bank.public.accounts WHERE id=1"), vec![vec![Value::String("fresh".into())]]);
}

#[test]
fn empty_retirement_and_further_legacy_mutations_are_refused_without_writes() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    let before = db.last_lsn();
    assert!(db.retire_legacy_schema_intents().is_err());
    assert_eq!(db.last_lsn(), before);
    prepare(&db, "bank.public.accounts");
    db.retire_legacy_schema_intents().unwrap();
    let before = db.last_lsn();
    assert!(db.abort_prepared_schema("bank.public.accounts").is_err());
    assert!(db.coordinate_prepared_schema("bank.public.accounts").is_err());
    assert!(db.certify_schema_readiness("bank.public.accounts").is_err());
    let (mut table, _) = db.schema_as_of("bank.public.accounts", before).unwrap();
    table.columns.push(Column { name: "more".into(), data_type: DataType::Text, nullable: true });
    assert!(db.prepare_nullable_addition("bank.public.accounts", &table).is_err());
    assert_eq!(db.last_lsn(), before);
    // Retirement may precede the explicit G35 capability transition.
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD fresh TEXT").unwrap();
}

#[test]
fn retirement_survives_restore_retention_and_multiple_active_revisions() {
    let (dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    prepare(&db, "bank.public.accounts");
    db.coordinate_prepared_schema("bank.public.accounts").unwrap();
    db.certify_schema_readiness("bank.public.accounts").unwrap();
    db.retire_legacy_schema_intents().unwrap();
    db.backup_to(dir.path().join("archive")).unwrap();
    restore_history_database(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let mut restored = HistorySqlEngine::open(dir.path().join("restored")).unwrap();
    assert!(restored.legacy_schema_intents_retired());
    restored.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    let floor = modify(&restored, &["UPDATE bank.public.accounts SET memo='kept' WHERE id=1"]);
    drop(restored);
    let raw = DurableMvccStore::open(dir.path().join("restored/history-domain")).unwrap();
    raw.prune_through(floor).unwrap();
    raw.checkpoint().unwrap();
    drop(raw);
    let mut restored = HistorySqlEngine::open(dir.path().join("restored")).unwrap();
    assert!(restored.legacy_schema_intents_retired());
    assert_eq!(query(&restored, "SELECT memo FROM bank.public.accounts WHERE id=1"), vec![vec![Value::String("kept".into())]]);
    restored.execute_ddl("ALTER TABLE bank.public.accounts ADD score INTEGER").unwrap();
    assert_eq!(query(&restored, "SELECT score FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null]]);
    assert_eq!(db.schema_as_of("bank.public.accounts", db.last_lsn()).unwrap().0.columns.len(), 2);
}

#[test]
fn changed_deleted_or_extended_legacy_history_invalidates_retirement() {
    for defect in ["rewrite", "delete", "new_legacy"] {
        let (_dir, root) = migrated();
        let mut db = HistorySqlEngine::open(&root).unwrap();
        prepare(&db, "bank.public.accounts");
        db.retire_legacy_schema_intents().unwrap();
        let (mut messages, _) = db.schema_as_of("bank.public.messages", db.last_lsn()).unwrap();
        messages.columns.push(Column { name: "illegal".into(), data_type: DataType::Text, nullable: true });
        drop(db);
        let raw = DurableMvccStore::open(root.join("history-domain")).unwrap();
        let mut tx = raw.begin();
        match defect {
            "rewrite" => {
                let value = tx.get(RETIREMENT).unwrap().unwrap();
                tx.replace(RETIREMENT, &value).unwrap();
            },
            "delete" => tx.delete(RETIREMENT).unwrap(),
            "new_legacy" => {
                let mut key = b"\0bradian-schema-proposal-v1:".to_vec();
                key.extend_from_slice(&messages.id.0.to_be_bytes());
                tx.insert(&key, &serde_json::to_vec(&messages).unwrap()).unwrap();
            },
            _ => unreachable!(),
        }
        tx.commit().unwrap();
        drop(raw);
        assert!(HistorySqlEngine::open(&root).is_err(), "accepted {defect}");
    }
}

#[test]
fn partial_retirement_frame_keeps_original_fences_and_allows_retry() {
    for cut in [0_u64, 1, 4, 17, u64::MAX] {
        let (_dir, root) = migrated();
        let mut db = HistorySqlEngine::open(&root).unwrap();
        prepare(&db, "bank.public.accounts");
        db.coordinate_prepared_schema("bank.public.accounts").unwrap();
        let before = db.last_lsn();
        db.checkpoint().unwrap();
        db.retire_legacy_schema_intents().unwrap();
        drop(db);
        let file = std::fs::OpenOptions::new().write(true).open(root.join("history-domain/rows.wal")).unwrap();
        file.set_len(cut.min(file.metadata().unwrap().len() - 1)).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let mut db = HistorySqlEngine::open(&root).unwrap();
        assert!(!db.legacy_schema_intents_retired());
        assert_eq!(db.last_lsn(), before);
        assert_eq!(db.prepared_schema_proposals().len(), 1);
        assert_eq!(db.retire_legacy_schema_intents().unwrap().history_lsn, before + 1);
        db.enable_epoch_row_writes().unwrap();
        db.execute_ddl("ALTER TABLE bank.public.accounts ADD after_retry TEXT").unwrap();
    }
}

#[test]
fn retirement_does_not_waive_catalog_horizon_checks() {
    let (_dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    prepare(&db, "bank.public.accounts");
    db.coordinate_prepared_schema("bank.public.accounts").unwrap();
    db.retire_legacy_schema_intents().unwrap();
    drop(db);
    let catalog = PersistentCatalog::open(root.join("catalog")).unwrap();
    catalog.create_database("out_of_band").unwrap();
    drop(catalog);
    assert!(HistorySqlEngine::open(&root).is_err());
}
