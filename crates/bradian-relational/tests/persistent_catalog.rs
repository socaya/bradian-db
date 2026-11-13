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

#[test]
fn g33_proposed_revision_replays_without_changing_live_table() {
    let dir = tempdir().unwrap();
    let id;
    let proposal_lsn;
    {
        let catalog = PersistentCatalog::open(dir.path()).unwrap();
        let db = catalog.create_database("clinic").unwrap();
        let schema = catalog.create_schema(db, "public").unwrap();
        id = catalog.create_table(schema, table("encounters")).unwrap();
        let live = catalog.snapshot().table(id).unwrap().clone();
        let mut proposed = live.clone();
        proposed.columns.push(Column {
            name: "notes".into(), data_type: DataType::Text, nullable: true,
        });
        proposal_lsn = catalog.propose_table_revision(id, proposed.clone()).unwrap();
        assert_eq!(catalog.proposed_table_revision(id), Some(proposed));
        assert_eq!(catalog.snapshot().table(id), Some(&live));
        assert!(catalog.propose_table_revision(id, live.clone()).is_err());
        assert_eq!(catalog.last_lsn(), proposal_lsn);
        assert!(catalog.drop_table(id).is_err());
        catalog.checkpoint().unwrap();
    }
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(catalog.last_lsn(), proposal_lsn);
    assert_eq!(catalog.proposed_table_revisions().len(), 1);
    assert_eq!(catalog.proposed_table_revision(id).unwrap().columns.len(), 3);
    assert_eq!(catalog.snapshot().table(id).unwrap().columns.len(), 2);
    assert!(catalog.drop_table(id).is_err());
}

#[test]
fn g33_rejects_unsafe_revisions_without_persisting_events() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let database = catalog.create_database("clinic").unwrap();
    let schema = catalog.create_schema(database, "public").unwrap();
    let id = catalog.create_table(schema, table("encounters")).unwrap();
    let original = catalog.snapshot().table(id).unwrap().clone();
    let before = catalog.last_lsn();
    let mut required = original.clone();
    required.columns.push(Column {
        name: "required".into(), data_type: DataType::Text, nullable: false,
    });
    assert!(catalog.propose_table_revision(id, required).is_err());
    let mut changed = original.clone();
    changed.columns[1].data_type = DataType::Int32;
    changed.columns.push(Column {
        name: "extra".into(), data_type: DataType::Text, nullable: true,
    });
    assert!(catalog.propose_table_revision(id, changed).is_err());
    let mut renamed = original.clone();
    renamed.name = "renamed".into();
    renamed.columns.push(Column {
        name: "extra".into(), data_type: DataType::Text, nullable: true,
    });
    assert!(catalog.propose_table_revision(id, renamed).is_err());
    assert_eq!(catalog.last_lsn(), before);
    assert!(catalog.proposed_table_revisions().is_empty());
}

fn revision_fixture(catalog: &PersistentCatalog) -> (bradian_relational::TableId, bradian_relational::Table) {
    let database = catalog.create_database("revisions").unwrap();
    let schema = catalog.create_schema(database, "public").unwrap();
    let id = catalog.create_table(schema, table("items")).unwrap();
    let mut proposed = catalog.snapshot().table(id).unwrap().clone();
    proposed.columns.push(Column { name: "notes".into(), data_type: DataType::Text, nullable: true });
    (id, proposed)
}

#[test]
fn revision_activation_replays_checkpoint_and_wal_without_changing_old_snapshots() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let (id, proposed) = revision_fixture(&catalog);
    let snapshot = catalog.snapshot();
    let lsn = catalog.propose_table_revision(id, proposed.clone()).unwrap();
    let revision = catalog.pending_table_revision(id).unwrap();
    assert_eq!(revision.proposal_lsn, lsn);
    catalog.checkpoint().unwrap();
    assert_eq!(catalog.activate_table_revision(revision.clone()).unwrap(), lsn + 1);
    assert_eq!(catalog.snapshot().table(id), Some(&proposed));
    assert_eq!(snapshot.table(id).unwrap().columns.len(), 2);
    assert!(catalog.pending_table_revision(id).is_none());
    assert!(catalog.activate_table_revision(revision).is_err());
    assert_eq!(catalog.last_lsn(), lsn + 1);
    drop(catalog);
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(catalog.snapshot().table(id), Some(&proposed));
    assert!(catalog.pending_table_revision(id).is_none());
    catalog.checkpoint().unwrap();
    drop(catalog);
    assert_eq!(PersistentCatalog::open(dir.path()).unwrap().snapshot().table(id), Some(&proposed));
}

#[test]
fn revision_cancellation_survives_restore_and_allows_table_drop() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("source")).unwrap();
    let (id, proposed) = revision_fixture(&catalog);
    catalog.propose_table_revision(id, proposed).unwrap();
    let revision = catalog.pending_table_revision(id).unwrap();
    let lsn = catalog.cancel_table_revision(revision.clone()).unwrap();
    assert!(catalog.cancel_table_revision(revision).is_err());
    assert_eq!(catalog.last_lsn(), lsn);
    catalog.backup_catalog_to(dir.path().join("backup")).unwrap();
    RowStore::restore_backup(dir.path().join("backup"), dir.path().join("restored")).unwrap();
    let restored = PersistentCatalog::open(dir.path().join("restored")).unwrap();
    assert!(restored.pending_table_revision(id).is_none());
    assert_eq!(restored.snapshot().table(id).unwrap().columns.len(), 2);
    restored.drop_table(id).unwrap();
}

#[test]
fn stale_and_mismatched_revision_decisions_leave_journal_unchanged() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let (id, proposed) = revision_fixture(&catalog);
    catalog.propose_table_revision(id, proposed.clone()).unwrap();
    let old = catalog.pending_table_revision(id).unwrap();
    catalog.cancel_table_revision(old.clone()).unwrap();
    catalog.propose_table_revision(id, proposed).unwrap();
    let current = catalog.pending_table_revision(id).unwrap();
    let before = catalog.last_lsn();
    // Same definition, different proposal: the old receipt must not decide it.
    assert!(catalog.activate_table_revision(old.clone()).is_err());
    assert!(catalog.cancel_table_revision(old).is_err());
    let mut forged = current.clone();
    forged.proposed.columns.last_mut().unwrap().name = "different".into();
    assert!(catalog.activate_table_revision(forged).is_err());
    assert_eq!(catalog.last_lsn(), before);
    catalog.activate_table_revision(current).unwrap();
}

#[test]
fn successive_revisions_preserve_identity_and_backup_active_definition() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("source")).unwrap();
    let (id, first) = revision_fixture(&catalog);
    catalog.propose_table_revision(id, first.clone()).unwrap();
    catalog.activate_table_revision(catalog.pending_table_revision(id).unwrap()).unwrap();
    let mut second = first;
    second.columns.push(Column { name: "tag".into(), data_type: DataType::Int64, nullable: true });
    catalog.propose_table_revision(id, second.clone()).unwrap();
    catalog.activate_table_revision(catalog.pending_table_revision(id).unwrap()).unwrap();
    catalog.backup_catalog_to(dir.path().join("backup")).unwrap();
    RowStore::restore_backup(dir.path().join("backup"), dir.path().join("restored")).unwrap();
    let restored = PersistentCatalog::open(dir.path().join("restored")).unwrap();
    assert_eq!(restored.snapshot().table(id), Some(&second));
    let next = restored.create_database("later").unwrap();
    assert_eq!(next.0, id.0 + 1);
}

#[test]
fn revision_proposals_reject_invalid_catalog_names_and_types() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let (id, proposed) = revision_fixture(&catalog);
    let before = catalog.last_lsn();
    for name in ["", "MixedCase", "id", "invalid name"] {
        let mut invalid = proposed.clone();
        invalid.columns.last_mut().unwrap().name = name.into();
        assert!(catalog.propose_table_revision(id, invalid).is_err());
    }
    for data_type in [DataType::Varchar { max_length: 0 }, DataType::Decimal { precision: 0, scale: 0 }] {
        let mut invalid = proposed.clone();
        invalid.columns.last_mut().unwrap().data_type = data_type;
        assert!(catalog.propose_table_revision(id, invalid).is_err());
    }
    assert_eq!(catalog.last_lsn(), before);
    assert!(catalog.proposed_table_revisions().is_empty());
}

#[test]
fn concurrent_activation_and_cancellation_publish_one_decision() {
    use std::sync::{Arc, Barrier};
    let dir = tempdir().unwrap();
    let catalog = Arc::new(PersistentCatalog::open(dir.path()).unwrap());
    let (id, proposed) = revision_fixture(&catalog);
    let before = catalog.propose_table_revision(id, proposed.clone()).unwrap();
    let revision = catalog.pending_table_revision(id).unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8).map(|i| {
        let catalog = Arc::clone(&catalog);
        let barrier = Arc::clone(&barrier);
        let revision = revision.clone();
        std::thread::spawn(move || {
            barrier.wait();
            if i % 2 == 0 { catalog.activate_table_revision(revision) }
            else { catalog.cancel_table_revision(revision) }
        })
    }).collect();
    let successes: Vec<_> = handles.into_iter().filter_map(|h| h.join().unwrap().ok()).collect();
    assert_eq!(successes, vec![before + 1]);
    let live = catalog.snapshot().table(id).unwrap().clone();
    assert!(live == proposed || live.columns.len() == 2);
    drop(catalog);
    let reopened = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(reopened.snapshot().table(id), Some(&live));
    assert!(reopened.pending_table_revision(id).is_none());
}

#[test]
fn forged_revision_decision_is_rejected_on_replay() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let (id, proposed) = revision_fixture(&catalog);
    catalog.propose_table_revision(id, proposed).unwrap();
    let mut revision = catalog.pending_table_revision(id).unwrap();
    revision.proposal_lsn -= 1;
    drop(catalog);
    let raw = RowStore::open(dir.path()).unwrap();
    let next = raw.last_lsn() + 1;
    let event = serde_json::json!({"version": 1, "result_id": null,
        "command": {"ActivateTableRevision": {"revision": revision}}});
    raw.put(format!("ddl/{next:016x}").as_bytes(), &serde_json::to_vec(&event).unwrap()).unwrap();
    drop(raw);
    assert!(matches!(PersistentCatalog::open(dir.path()), Err(PersistentCatalogError::Corruption(_))));
}

#[test]
fn pending_revision_fences_constraint_changes_without_wal_append() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let (id, proposed) = revision_fixture(&catalog);
    let before = catalog.propose_table_revision(id, proposed).unwrap();
    assert!(catalog.add_foreign_key(id, ForeignKeySpec {
        name: "self_ref".into(), columns: vec!["id".into()],
        referenced_table: id, referenced_columns: vec!["id".into()],
    }).is_err());
    assert_eq!(catalog.last_lsn(), before);
    assert!(catalog.snapshot().table(id).unwrap().foreign_keys.is_empty());
}


#[test]
fn torn_revision_decision_retains_pending_state_and_can_be_retried() {
    // Exercise both decisions at header, payload and checksum truncations.
    for activate in [true, false] {
        for cut in [0_u64, 1, 4, 17, 100, u64::MAX] {
            let dir = tempdir().unwrap();
            let catalog = PersistentCatalog::open(dir.path()).unwrap();
            let (id, proposed) = revision_fixture(&catalog);
            let before = catalog.propose_table_revision(id, proposed).unwrap();
            let revision = catalog.pending_table_revision(id).unwrap();
            catalog.checkpoint().unwrap();
            if activate { catalog.activate_table_revision(revision.clone()).unwrap(); }
            else { catalog.cancel_table_revision(revision.clone()).unwrap(); }
            drop(catalog);
            let wal = std::fs::OpenOptions::new().write(true).open(dir.path().join("rows.wal")).unwrap();
            let length = wal.metadata().unwrap().len();
            assert!(length > 100);
            wal.set_len(cut.min(length - 1)).unwrap();
            wal.sync_all().unwrap();
            drop(wal);
            let catalog = PersistentCatalog::open(dir.path()).unwrap();
            assert_eq!(catalog.last_lsn(), before);
            assert_eq!(catalog.pending_table_revision(id), Some(revision.clone()));
            assert_eq!(catalog.snapshot().table(id).unwrap().columns.len(), 2);
            // Recovery repairs the partial tail before accepting another write.
            if activate { catalog.activate_table_revision(revision).unwrap(); }
            else { catalog.cancel_table_revision(revision).unwrap(); }
            drop(catalog);
            let catalog = PersistentCatalog::open(dir.path()).unwrap();
            assert_eq!(catalog.last_lsn(), before + 1);
            assert!(catalog.pending_table_revision(id).is_none());
            assert_eq!(catalog.snapshot().table(id).unwrap().columns.len(), if activate { 3 } else { 2 });
        }
    }
}
