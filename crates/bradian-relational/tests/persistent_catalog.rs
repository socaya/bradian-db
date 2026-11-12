//! File-backed catalog DDL journal: real WAL, restart, checkpoint, constraints.

use bradian_relational::{
    CatalogError, Column, DataType, ForeignKeySpec, PrimaryKey, TableSpec,
};
use bradian_relational::persistent_catalog::{
    PersistentCatalog, PersistentCatalogError,
};
use bradian_relational::row_store::RowStore;
use tempfile::tempdir;

fn table(name: &str) -> TableSpec {
    TableSpec {
        name: name.into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "description".into(),
                data_type: DataType::Varchar { max_length: 160 }, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }
}

#[test]
fn database_schema_and_table_survive_restart_with_stable_ids() {
    let dir = tempdir().unwrap();
    let (database, schema, table_id);
    {
        let db = PersistentCatalog::open(dir.path()).unwrap();
        database = db.create_database("Billing").unwrap();
        schema = db.create_schema(database, "Public").unwrap();
        table_id = db.create_table(schema, table("Invoices")).unwrap();
        assert_eq!(db.last_lsn(), 4); // one version marker, three DDL records
        assert_eq!(db.snapshot().table(table_id).unwrap().name, "invoices");
    }
    let reopened = PersistentCatalog::open(dir.path()).unwrap();
    let view = reopened.snapshot();
    assert_eq!(view.database(database).unwrap().name, "billing");
    assert_eq!(view.schema(schema).unwrap().name, "public");
    assert_eq!(view.table(table_id).unwrap().name, "invoices");
    assert_eq!(reopened.last_lsn(), 4);
    let second_id = reopened.create_table(schema, table("payments")).unwrap();
    assert!(second_id.0 > table_id.0);
    drop(reopened);
    assert_eq!(PersistentCatalog::open(dir.path()).unwrap()
        .snapshot().table(second_id).unwrap().name, "payments");
}

#[test]
fn rejected_ddl_does_not_append_a_wal_frame_or_advance_identity() {
    let dir = tempdir().unwrap();
    let db = PersistentCatalog::open(dir.path()).unwrap();
    let a = db.create_database("prod").unwrap();
    let lsn = db.last_lsn();
    assert!(matches!(db.create_database("PROD"),
        Err(PersistentCatalogError::Catalog(CatalogError::Duplicate(_)))));
    assert_eq!(db.last_lsn(), lsn);
    let b = db.create_database("test").unwrap();
    assert_eq!(b.0, a.0 + 1);
    assert_eq!(db.last_lsn(), lsn + 1);
}

#[test]
fn foreign_keys_and_restrict_deletes_remain_valid_after_replay() {
    let dir = tempdir().unwrap();
    let (database, scope, parent, child);
    {
        let db = PersistentCatalog::open(dir.path()).unwrap();
        database = db.create_database("hospital").unwrap();
        scope = db.create_schema(database, "clinical").unwrap();
        parent = db.create_table(scope, table("patients")).unwrap();
        let mut child_spec = table("encounters");
        child_spec.columns.push(Column {
            name: "patient_id".into(),
            data_type: DataType::Int64,
            nullable: false,
        });
        child = db.create_table(scope, child_spec).unwrap();
        db.add_foreign_key(child, ForeignKeySpec {
            name: "fk_encounter_patient".into(),
            columns: vec!["patient_id".into()],
            referenced_table: parent,
            referenced_columns: vec!["id".into()],
        }).unwrap();
        db.checkpoint().unwrap();
    }
    let db = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(db.snapshot().table(child).unwrap().foreign_keys.len(), 1);
    assert!(matches!(db.drop_table(parent),
        Err(PersistentCatalogError::Catalog(CatalogError::DependentObjects(_)))));
    db.drop_table(child).unwrap();
    db.drop_table(parent).unwrap();
    db.drop_schema(scope).unwrap();
    db.drop_database(database).unwrap();
    drop(db);
    let reopened = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(reopened.snapshot().databases().count(), 0);
}

#[test]
fn dropping_object_does_not_reuse_its_persisted_identity() {
    let dir = tempdir().unwrap();
    let id;
    {
        let db = PersistentCatalog::open(dir.path()).unwrap();
        id = db.create_database("temporary").unwrap();
        db.drop_database(id).unwrap();
    }
    let db = PersistentCatalog::open(dir.path()).unwrap();
    let after = db.create_database("another").unwrap();
    assert!(after.0 > id.0);
}

#[test]
fn journal_recovers_checkpoint_and_later_wal_events() {
    let dir = tempdir().unwrap();
    let (database, schema, created);
    {
        let db = PersistentCatalog::open(dir.path()).unwrap();
        database = db.create_database("production").unwrap();
        db.checkpoint().unwrap();
        schema = db.create_schema(database, "public").unwrap();
        created = db.create_table(schema, table("records")).unwrap();
    }
    let db = PersistentCatalog::open(dir.path()).unwrap();
    assert!(db.snapshot().database(database).is_some());
    assert!(db.snapshot().schema(schema).is_some());
    assert!(db.snapshot().table(created).is_some());
    assert_eq!(db.last_lsn(), 4);
    db.checkpoint().unwrap();
    drop(db);
    assert_eq!(PersistentCatalog::open(dir.path()).unwrap().last_lsn(), 4);
}

#[test]
fn concurrent_metadata_writers_are_serialized_and_names_unique() {
    use std::sync::Arc;
    use std::thread;

    let dir = tempdir().unwrap();
    let catalog = Arc::new(PersistentCatalog::open(dir.path()).unwrap());
    let handles = (0..12).map(|_| {
        let cat = Arc::clone(&catalog);
        thread::spawn(move || cat.create_database("same_name"))
    }).collect::<Vec<_>>();
    let mut successes = 0;
    for handle in handles {
        match handle.join().unwrap() {
            Ok(_) => successes += 1,
            Err(PersistentCatalogError::Catalog(CatalogError::Duplicate(_))) => {},
            other => panic!("unexpected DDL result: {other:?}"),
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(catalog.snapshot().databases().count(), 1);
    assert_eq!(catalog.last_lsn(), 2);
}

#[test]
fn second_process_catalog_open_is_blocked_by_underlying_store_lock() {
    let dir = tempdir().unwrap();
    let first = PersistentCatalog::open(dir.path()).unwrap();
    assert!(matches!(PersistentCatalog::open(dir.path()),
        Err(PersistentCatalogError::Storage(_))));
    drop(first);
    assert!(PersistentCatalog::open(dir.path()).is_ok());
}

#[test]
fn corrupt_persisted_event_fails_closed_instead_of_skipping_ddl() {
    let dir = tempdir().unwrap();
    {
        let catalog = PersistentCatalog::open(dir.path()).unwrap();
        catalog.create_database("first").unwrap();
    }
    {
        let raw = RowStore::open(dir.path()).unwrap();
        raw.put(b"ddl/0000000000000002", b"{not-valid-json").unwrap();
    }
    let failure = PersistentCatalog::open(dir.path());
    assert!(matches!(failure, Err(PersistentCatalogError::Corruption(_))));
}

#[test]
fn extra_unrecognized_catalog_key_is_rejected() {
    let dir = tempdir().unwrap();
    {
        let catalog = PersistentCatalog::open(dir.path()).unwrap();
        catalog.create_database("one").unwrap();
    }
    {
        let raw = RowStore::open(dir.path()).unwrap();
        raw.put(b"malicious-extra-row", b"untracked").unwrap();
    }
    assert!(matches!(PersistentCatalog::open(dir.path()),
        Err(PersistentCatalogError::Corruption(_))));
}

#[test]
fn incorrect_journal_version_marker_is_rejected() {
    let dir = tempdir().unwrap();
    {
        let catalog = PersistentCatalog::open(dir.path()).unwrap();
        catalog.create_database("one").unwrap();
    }
    {
        let raw = RowStore::open(dir.path()).unwrap();
        raw.put(b"bradian.catalog.version", b"BRADIAN-CATALOG-DDL-V99").unwrap();
    }
    assert!(matches!(PersistentCatalog::open(dir.path()),
        Err(PersistentCatalogError::Corruption(_))));
}

#[test]
fn oversized_ddl_is_refused_before_commit() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let db = catalog.create_database("prod").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    let lsn = catalog.last_lsn();
    let mut spec = table("wide_table");
    for i in 0..140 {
        spec.columns.push(Column {
            name: format!("field_{i:03}"),
            data_type: DataType::Text,
            nullable: true,
        });
    }
    assert!(matches!(catalog.create_table(schema, spec),
        Err(PersistentCatalogError::Storage(_))));
    assert_eq!(catalog.last_lsn(), lsn);
    assert_eq!(catalog.snapshot().tables().count(), 0);
}

#[test]
fn snapshots_are_independent_of_subsequent_commits() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let before = catalog.snapshot();
    let id = catalog.create_database("added").unwrap();
    assert_eq!(before.databases().count(), 0);
    assert!(catalog.snapshot().database(id).is_some());
}


#[test]
fn catalog_only_backup_preserves_replayable_metadata_journal() {
    let base = tempdir().unwrap();
    let root = base.path().join("catalog");
    let backup = base.path().join("catalog-backup");
    let restored = base.path().join("catalog-restored");
    let (database, schema, table_id);
    {
        let catalog = PersistentCatalog::open(&root).unwrap();
        database = catalog.create_database("reports").unwrap();
        schema = catalog.create_schema(database, "public").unwrap();
        table_id = catalog.create_table(schema, table("summaries")).unwrap();
        let receipt = catalog.backup_catalog_to(&backup).unwrap();
        assert_eq!(receipt.last_lsn, catalog.last_lsn());
    }
    RowStore::restore_backup(&backup, &restored).unwrap();
    let recovered = PersistentCatalog::open(&restored).unwrap();
    assert_eq!(recovered.snapshot().database(database).unwrap().name, "reports");
    assert_eq!(recovered.snapshot().schema(schema).unwrap().name, "public");
    assert_eq!(recovered.snapshot().table(table_id).unwrap().name, "summaries");
}

#[test]
fn catalog_only_backup_does_not_advance_metadata_identity_counter() {
    let base = tempdir().unwrap();
    let root = base.path().join("catalog");
    let catalog = PersistentCatalog::open(&root).unwrap();
    let first = catalog.create_database("first").unwrap();
    let lsn_before = catalog.last_lsn();
    catalog.backup_catalog_to(base.path().join("backup")).unwrap();
    assert_eq!(catalog.last_lsn(), lsn_before);
    let second = catalog.create_database("second").unwrap();
    assert_eq!(second.0, first.0 + 1);
}
