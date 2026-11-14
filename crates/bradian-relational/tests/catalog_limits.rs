//! Bounded admission for persistent-catalog decoded state (ADR 0054).
//!
//! The catalog journal is append-only until a versioned compactor is designed.
//! These tests prove that decoded-state growth is admitted up front and that a
//! refusal does not touch the journal, poison the live catalog or corrupt an
//! existing journal against a tighter replay policy.

use bradian_relational::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use bradian_relational::runtime_limits::{CatalogLimits, ResourceError};
use bradian_relational::{Column, DataType, PrimaryKey, TableSpec};
use tempfile::tempdir;

fn table(name: &str) -> TableSpec {
    TableSpec {
        name: name.into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: Vec::new(),
    }
}

fn quota(err: PersistentCatalogError, resource: &str) {
    match err {
        PersistentCatalogError::Resource(ResourceError::CatalogLimit { resource: actual, .. }) => {
            assert_eq!(actual, resource);
        }
        other => panic!("expected CatalogLimit({resource}), got {other:?}"),
    }
}

#[test]
fn live_commit_refuses_additional_databases_beyond_the_configured_limit() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open_with_limits(
        dir.path(),
        CatalogLimits { max_databases: 2, ..CatalogLimits::default() },
    ).unwrap();
    catalog.create_database("alpha").unwrap();
    catalog.create_database("beta").unwrap();
    let lsn_before = catalog.last_lsn();
    let usage_before = catalog.resource_usage();
    quota(catalog.create_database("gamma").unwrap_err(), "databases");
    // Refusal preserved the live catalog and journal state exactly.
    assert_eq!(catalog.last_lsn(), lsn_before);
    assert_eq!(catalog.resource_usage(), usage_before);
    assert_eq!(catalog.snapshot().databases().count(), 2);
    // A relaxed reopen replays only the two committed databases.
    drop(catalog);
    let relaxed = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(relaxed.snapshot().databases().count(), 2);
}

#[test]
fn live_commit_refuses_additional_schemas_and_tables_beyond_the_limit() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open_with_limits(
        dir.path(),
        CatalogLimits {
            max_schemas: 1,
            max_tables: 2,
            ..CatalogLimits::default()
        },
    ).unwrap();
    let db = catalog.create_database("app").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    quota(catalog.create_schema(db, "other").unwrap_err(), "schemas");
    catalog.create_table(schema, table("t1")).unwrap();
    catalog.create_table(schema, table("t2")).unwrap();
    quota(catalog.create_table(schema, table("t3")).unwrap_err(), "tables");
    // Schema count remains at one and table count at two after the refusals.
    assert_eq!(catalog.resource_usage().schemas, 1);
    assert_eq!(catalog.resource_usage().tables, 2);
}

#[test]
fn opening_with_tighter_limits_than_committed_state_fails_without_modifying_the_journal() {
    let dir = tempdir().unwrap();
    {
        let catalog = PersistentCatalog::open(dir.path()).unwrap();
        catalog.create_database("one").unwrap();
        catalog.create_database("two").unwrap();
    }
    let journal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    let err = PersistentCatalog::open_with_limits(
        dir.path(),
        CatalogLimits { max_databases: 1, ..CatalogLimits::default() },
    ).unwrap_err();
    quota(err, "databases");
    // Refused replay must leave the WAL journal byte-identical.
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), journal_before);
    // Reopen with a sufficient policy still succeeds and sees both databases.
    let relaxed = PersistentCatalog::open(dir.path()).unwrap();
    assert_eq!(relaxed.snapshot().databases().count(), 2);
}

#[test]
fn oversized_ddl_event_is_refused_before_touching_the_wal() {
    let dir = tempdir().unwrap();
    // Set the event-byte budget small enough that any CreateDatabase event
    // (which serializes its name) exceeds it.
    let catalog = PersistentCatalog::open_with_limits(
        dir.path(),
        CatalogLimits { max_event_bytes: 1, ..CatalogLimits::default() },
    ).unwrap();
    let journal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    quota(catalog.create_database("anything").unwrap_err(), "event bytes");
    // Refusal leaves the WAL exactly as the catalog marker wrote it.
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), journal_before);
    assert_eq!(catalog.snapshot().databases().count(), 0);
}

#[test]
fn default_catalog_limits_admit_an_ordinary_working_catalog() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let db = catalog.create_database("ordinary").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    for i in 0..16 {
        catalog.create_table(schema, table(&format!("t{i}"))).unwrap();
    }
    let usage = catalog.resource_usage();
    assert_eq!(usage.databases, 1);
    assert_eq!(usage.schemas, 1);
    assert_eq!(usage.tables, 16);
}
