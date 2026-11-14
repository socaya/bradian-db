//! Bounded admission for MultiTableStore decoded state (ADR 0056).

use bradian_relational::transaction_domain::{DomainError, MultiTableStore};
use bradian_relational::runtime_limits::{DomainLimits, ResourceError};
use bradian_relational::{Catalog, Column, DataType, PrimaryKey, Table, TableSpec};
use tempfile::tempdir;

fn one_table(catalog: &mut Catalog, name: &str) -> Table {
    let existing_db = { catalog.databases().next().map(|d| d.id) };
    let db = match existing_db {
        Some(id) => id,
        None => catalog.create_database("test").unwrap(),
    };
    let existing_schema = { catalog.schemas().next().map(|s| s.id) };
    let schema = match existing_schema {
        Some(id) => id,
        None => catalog.create_schema(db, "public").unwrap(),
    };
    let id = catalog.create_table(schema, TableSpec {
        name: name.into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }).unwrap();
    catalog.table(id).unwrap().clone()
}

fn quota(err: DomainError, resource: &str) {
    match err {
        DomainError::Resource(ResourceError::DomainLimit { resource: actual, .. }) => {
            assert_eq!(actual, resource);
        }
        other => panic!("expected DomainLimit({resource}), got {other:?}"),
    }
}

#[test]
fn register_table_refuses_additional_tables_beyond_configured_limit() {
    let dir = tempdir().unwrap();
    let store = MultiTableStore::open_with_limits(
        dir.path(),
        DomainLimits { max_tables: 2 },
    ).unwrap();
    let mut catalog = Catalog::new();
    let a = one_table(&mut catalog, "a");
    let b = one_table(&mut catalog, "b");
    let c = one_table(&mut catalog, "c");
    store.register_table(a).unwrap();
    store.register_table(b).unwrap();
    assert_eq!(store.registered_table_count(), 2);
    let wal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    quota(store.register_table(c).unwrap_err(), "tables");
    // Refusal leaves the WAL exactly as it was and does not grow the map.
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), wal_before);
    assert_eq!(store.registered_table_count(), 2);
}

#[test]
fn idempotent_register_of_same_table_does_not_charge_additional_capacity() {
    let dir = tempdir().unwrap();
    let store = MultiTableStore::open_with_limits(
        dir.path(),
        DomainLimits { max_tables: 1 },
    ).unwrap();
    let mut catalog = Catalog::new();
    let a = one_table(&mut catalog, "a");
    store.register_table(a.clone()).unwrap();
    // Re-registering the same table must not trip admission.
    store.register_table(a).unwrap();
    assert_eq!(store.registered_table_count(), 1);
}

#[test]
fn opening_with_tighter_limits_than_committed_tables_fails_without_modifying_wal() {
    let dir = tempdir().unwrap();
    let mut catalog = Catalog::new();
    let a = one_table(&mut catalog, "a");
    let b = one_table(&mut catalog, "b");
    {
        let store = MultiTableStore::open(dir.path()).unwrap();
        store.register_table(a).unwrap();
        store.register_table(b).unwrap();
    }
    let wal_before = std::fs::read(dir.path().join("rows.wal")).unwrap();
    let err = MultiTableStore::open_with_limits(
        dir.path(),
        DomainLimits { max_tables: 1 },
    ).unwrap_err();
    quota(err, "tables");
    assert_eq!(std::fs::read(dir.path().join("rows.wal")).unwrap(), wal_before);
    // A relaxed reopen still sees both tables.
    let relaxed = MultiTableStore::open(dir.path()).unwrap();
    assert_eq!(relaxed.registered_table_count(), 2);
}

#[test]
fn default_limits_admit_an_ordinary_multi_table_domain() {
    let dir = tempdir().unwrap();
    let store = MultiTableStore::open(dir.path()).unwrap();
    let mut catalog = Catalog::new();
    for i in 0..8 {
        store.register_table(one_table(&mut catalog, &format!("t{i}"))).unwrap();
    }
    assert_eq!(store.registered_table_count(), 8);
    assert_eq!(store.resource_limits().max_tables, DomainLimits::default().max_tables);
}
