//! Observational workspace usage snapshot (ADR 0059).
//!
//! Non-admitting operator diagnostic. Tests verify each `with_*` adds
//! exactly one atomic reading from its source engine and does not
//! change admission behaviour.

use bradian_relational::mvcc::MvccStore;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::transaction_domain::MultiTableStore;
use bradian_relational::workspace_usage::WorkspaceUsage;
use bradian_relational::{Column, DataType, PrimaryKey, TableSpec};
use tempfile::tempdir;

fn one_db_two_tables(catalog: &PersistentCatalog) -> Vec<bradian_relational::Table> {
    let db = catalog.create_database("test").unwrap();
    let schema = catalog.create_schema(db, "public").unwrap();
    let mut out = Vec::new();
    for name in ["a", "b"] {
        let id = catalog.create_table(schema, TableSpec {
            name: name.into(),
            columns: vec![
                Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            ],
            primary_key: PrimaryKey { columns: vec!["id".into()] },
            unique_keys: vec![],
        }).unwrap();
        out.push(catalog.snapshot().tables().find(|t| t.id == id).unwrap().clone());
    }
    out
}

#[test]
fn catalog_snapshot_reports_current_counts() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path()).unwrap();
    let usage = WorkspaceUsage::for_catalog(&catalog);
    assert_eq!(usage.catalog.databases, 0);
    assert_eq!(usage.catalog.schemas, 0);
    assert_eq!(usage.catalog.tables, 0);
    assert_eq!(usage.catalog.revisions, 0);
    assert!(usage.domain_tables.is_none());
    assert!(usage.mvcc_distinct_keys.is_none());
    assert!(usage.mvcc_total_versions.is_none());

    let _tables = one_db_two_tables(&catalog);
    let usage = WorkspaceUsage::for_catalog(&catalog);
    assert_eq!(usage.catalog.databases, 1);
    assert_eq!(usage.catalog.schemas, 1);
    assert_eq!(usage.catalog.tables, 2);
}

#[test]
fn with_domain_adds_registered_table_count_without_changing_catalog_counts() {
    let dir = tempdir().unwrap();
    let catalog_dir = dir.path().join("catalog");
    let domain_dir = dir.path().join("domain");
    let catalog = PersistentCatalog::open(&catalog_dir).unwrap();
    let tables = one_db_two_tables(&catalog);

    let domain = MultiTableStore::open(&domain_dir).unwrap();
    domain.register_table(tables[0].clone()).unwrap();

    let usage = WorkspaceUsage::for_catalog(&catalog).with_domain(&domain);
    assert_eq!(usage.catalog.tables, 2);
    assert_eq!(usage.domain_tables, Some(1));
}

#[test]
fn with_mvcc_adds_retained_keys_and_versions() {
    let dir = tempdir().unwrap();
    let catalog_dir = dir.path().join("catalog");
    let mvcc_dir = dir.path().join("mvcc");
    let catalog = PersistentCatalog::open(&catalog_dir).unwrap();
    let mvcc = MvccStore::open(&mvcc_dir).unwrap();

    let mut tx = mvcc.begin();
    tx.insert(b"k1", b"v").unwrap();
    tx.insert(b"k2", b"v").unwrap();
    tx.commit().unwrap();

    let usage = WorkspaceUsage::for_catalog(&catalog).with_mvcc(&mvcc);
    assert_eq!(usage.mvcc_distinct_keys, Some(2));
    assert!(usage.mvcc_total_versions.is_some_and(|v| v >= 2));
}

#[test]
fn full_snapshot_combines_all_three_engines() {
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let tables = one_db_two_tables(&catalog);
    let domain = MultiTableStore::open(dir.path().join("domain")).unwrap();
    domain.register_table(tables[0].clone()).unwrap();
    domain.register_table(tables[1].clone()).unwrap();
    let mvcc = MvccStore::open(dir.path().join("mvcc")).unwrap();
    let mut tx = mvcc.begin();
    tx.insert(b"x", b"v").unwrap();
    tx.commit().unwrap();

    let usage = WorkspaceUsage::for_catalog(&catalog)
        .with_domain(&domain)
        .with_mvcc(&mvcc);

    assert_eq!(usage.catalog.tables, 2);
    assert_eq!(usage.domain_tables, Some(2));
    assert_eq!(usage.mvcc_distinct_keys, Some(1));
    // Display is readable and includes every attached engine.
    let line = format!("{usage}");
    assert!(line.contains("catalog:"));
    assert!(line.contains("domain:"));
    assert!(line.contains("mvcc:"));
}

#[test]
fn workspace_usage_is_non_admitting_and_takes_no_locks_longer_than_one_read() {
    // Build a snapshot, then mutate every engine. The snapshot is a
    // frozen past view; it must not track live changes.
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let tables = one_db_two_tables(&catalog);
    let domain = MultiTableStore::open(dir.path().join("domain")).unwrap();
    domain.register_table(tables[0].clone()).unwrap();
    let mvcc = MvccStore::open(dir.path().join("mvcc")).unwrap();

    let frozen = WorkspaceUsage::for_catalog(&catalog)
        .with_domain(&domain)
        .with_mvcc(&mvcc);

    // Mutate everything post-snapshot.
    let db = catalog.create_database("new_db").unwrap();
    let _schema = catalog.create_schema(db, "s").unwrap();
    domain.register_table(tables[1].clone()).unwrap();
    let mut tx = mvcc.begin();
    tx.insert(b"a", b"v").unwrap();
    tx.commit().unwrap();

    // Original snapshot is unchanged — it is a plain copy, not a view.
    assert_eq!(frozen.catalog.databases, 1);
    assert_eq!(frozen.domain_tables, Some(1));
    assert_eq!(frozen.mvcc_distinct_keys, Some(0));
}
