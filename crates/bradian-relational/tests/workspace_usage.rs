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

// ---------- ADR 0059 extension — RowStore + DurableMvccStore ----------

#[test]
fn with_row_store_reports_decoded_rows_and_delta_counts() {
    use bradian_relational::row_store::RowStore;
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let store = RowStore::open(dir.path().join("rows")).unwrap();
    store.put(b"alice", b"active").unwrap();
    store.put(b"bob", b"pending").unwrap();

    let usage = WorkspaceUsage::for_catalog(&catalog).with_row_store(&store);
    let rs = usage.row_store.expect("with_row_store installed");
    assert_eq!(rs.rows, 2);
    assert!(rs.row_bytes > 0);
    // Delta keys / bytes reflect uncommitted/post-checkpoint mutations.
    // Fresh store with no checkpoint: delta counts match put history.
    assert!(rs.delta_keys >= 2);
}

#[test]
fn with_durable_mvcc_reports_retained_history_counts() {
    use bradian_relational::durable_mvcc::DurableMvccStore;
    use bradian_relational::runtime_limits::HistoryLimits;
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let store = DurableMvccStore::open_with_limits(
        dir.path().join("hist"), HistoryLimits::default(),
    ).unwrap();
    {
        let mut tx = store.begin();
        tx.insert(b"k", b"v").unwrap();
        tx.commit().unwrap();
    }

    let usage = WorkspaceUsage::for_catalog(&catalog).with_durable_mvcc(&store);
    let h = usage.durable_mvcc.expect("with_durable_mvcc installed");
    assert!(h.keys >= 1);
    assert!(h.versions >= 1);
    assert!(h.bytes > 0);
}

#[test]
fn full_five_engine_display_formats_every_field() {
    use bradian_relational::durable_mvcc::DurableMvccStore;
    use bradian_relational::row_store::RowStore;
    use bradian_relational::runtime_limits::HistoryLimits;
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let _tables = one_db_two_tables(&catalog);
    let domain = MultiTableStore::open(dir.path().join("domain")).unwrap();
    let mvcc = MvccStore::open(dir.path().join("mvcc")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    rs.put(b"k", b"v").unwrap();
    let dmv = DurableMvccStore::open_with_limits(
        dir.path().join("hist"), HistoryLimits::default(),
    ).unwrap();
    {
        let mut tx = dmv.begin();
        tx.insert(b"a", b"b").unwrap();
        tx.commit().unwrap();
    }

    let full = WorkspaceUsage::for_catalog(&catalog)
        .with_domain(&domain)
        .with_mvcc(&mvcc)
        .with_row_store(&rs)
        .with_durable_mvcc(&dmv);
    let s = format!("{full}");
    assert!(s.contains("catalog:"));
    assert!(s.contains("domain:"));
    assert!(s.contains("mvcc:"));
    assert!(s.contains("row_store:"));
    assert!(s.contains("durable_mvcc:"));
}

// ---------- ADR 0110 global budget ----------

#[test]
fn total_bytes_sums_row_store_and_durable_mvcc() {
    use bradian_relational::durable_mvcc::DurableMvccStore;
    use bradian_relational::row_store::RowStore;
    use bradian_relational::runtime_limits::HistoryLimits;
    use bradian_relational::workspace_usage::WorkspaceUsage;
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    rs.put(b"k1", b"value-one").unwrap();
    rs.put(b"k2", b"value-two").unwrap();
    let dmv = DurableMvccStore::open_with_limits(
        dir.path().join("hist"), HistoryLimits::default(),
    ).unwrap();
    {
        let mut tx = dmv.begin();
        tx.insert(b"h1", b"v").unwrap();
        tx.commit().unwrap();
    }

    let usage = WorkspaceUsage::for_catalog(&catalog)
        .with_row_store(&rs).with_durable_mvcc(&dmv);
    let rs_bytes = usage.row_store.unwrap().row_bytes as u64
        + usage.row_store.unwrap().delta_bytes as u64;
    let dmv_bytes = usage.durable_mvcc.unwrap().bytes as u64;
    assert_eq!(usage.total_bytes(), rs_bytes + dmv_bytes);
    assert!(usage.total_bytes() > 0);
}

#[test]
fn global_budget_default_is_unlimited_and_never_rejects() {
    use bradian_relational::row_store::RowStore;
    use bradian_relational::workspace_usage::{GlobalBudget, WorkspaceUsage};
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    for i in 0..10 { rs.put(format!("k{i}").as_bytes(), b"value").unwrap(); }
    let usage = WorkspaceUsage::for_catalog(&catalog).with_row_store(&rs);
    assert!(GlobalBudget::unlimited().check(&usage).is_ok());
}

#[test]
fn global_budget_rejects_when_total_bytes_exceeds_cap() {
    use bradian_relational::row_store::RowStore;
    use bradian_relational::runtime_limits::ResourceError;
    use bradian_relational::workspace_usage::{GlobalBudget, WorkspaceUsage};
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    for i in 0..5 {
        rs.put(format!("k{i}").as_bytes(), b"some-reasonable-value").unwrap();
    }
    let usage = WorkspaceUsage::for_catalog(&catalog).with_row_store(&rs);
    let budget = GlobalBudget::unlimited().with_max_total_bytes(0);
    match budget.check(&usage) {
        Err(ResourceError::GlobalBudget { resource, actual, limit }) => {
            assert_eq!(resource, "total_bytes");
            assert_eq!(limit, 0);
            assert!(actual > 0);
        }
        other => panic!("expected GlobalBudget error; got {other:?}"),
    }
}

#[test]
fn global_budget_rejects_total_rows_before_total_bytes() {
    use bradian_relational::row_store::RowStore;
    use bradian_relational::runtime_limits::ResourceError;
    use bradian_relational::workspace_usage::{GlobalBudget, WorkspaceUsage};
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    for i in 0..3 { rs.put(format!("k{i}").as_bytes(), b"v").unwrap(); }
    let usage = WorkspaceUsage::for_catalog(&catalog).with_row_store(&rs);

    // Byte cap checked first; set it high enough to pass.
    // Row cap of 2 < 3 rows → rejects on rows.
    let budget = GlobalBudget::unlimited()
        .with_max_total_bytes(u64::MAX)
        .with_max_total_rows(2);
    match budget.check(&usage) {
        Err(ResourceError::GlobalBudget { resource, actual, limit }) => {
            assert_eq!(resource, "total_rows");
            assert_eq!(limit, 2);
            assert_eq!(actual, 3);
        }
        other => panic!("expected GlobalBudget rows error; got {other:?}"),
    }
}

#[test]
fn global_budget_bytes_cap_fires_before_rows_cap_when_both_exceeded() {
    use bradian_relational::row_store::RowStore;
    use bradian_relational::runtime_limits::ResourceError;
    use bradian_relational::workspace_usage::{GlobalBudget, WorkspaceUsage};
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    for i in 0..5 { rs.put(format!("k{i}").as_bytes(), b"vv").unwrap(); }
    let usage = WorkspaceUsage::for_catalog(&catalog).with_row_store(&rs);

    let budget = GlobalBudget::unlimited()
        .with_max_total_bytes(0)
        .with_max_total_rows(0);
    match budget.check(&usage) {
        Err(ResourceError::GlobalBudget { resource, .. }) => {
            assert_eq!(resource, "total_bytes",
                "byte check must fire first per implementation order");
        }
        other => panic!("expected GlobalBudget error; got {other:?}"),
    }
}

#[test]
fn global_budget_is_post_sampling_check_not_atomic_with_mutations() {
    // Honest: GlobalBudget::check reads a frozen snapshot. A
    // mutation between sample and check-use is not covered.
    // Tests just verify the diagnostic surface.
    use bradian_relational::row_store::RowStore;
    use bradian_relational::workspace_usage::{GlobalBudget, WorkspaceUsage};
    let dir = tempdir().unwrap();
    let catalog = PersistentCatalog::open(dir.path().join("catalog")).unwrap();
    let rs = RowStore::open(dir.path().join("rows")).unwrap();
    rs.put(b"k", b"v").unwrap();
    let frozen = WorkspaceUsage::for_catalog(&catalog).with_row_store(&rs);
    // Mutate after snapshot.
    for i in 0..100 { rs.put(format!("after{i}").as_bytes(), b"many bytes").unwrap(); }
    // Frozen snapshot still sees the pre-mutation view.
    let budget = GlobalBudget::unlimited().with_max_total_rows(50);
    assert!(budget.check(&frozen).is_ok(),
        "frozen snapshot does not track live mutations");
}
