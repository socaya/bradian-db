//! ADR 0242 — per-table schema_version + SchemaSnapshot.

use bradian_relational::{
    Catalog, Column, DataType, PrimaryKey, SchemaSnapshot, SchemaStale, TableId, TableSpec,
};

fn fresh_catalog() -> (Catalog, TableId, TableId) {
    let mut cat = Catalog::new();
    let db = cat.create_database("app").unwrap();
    let sch = cat.create_schema(db, "public").unwrap();
    let t1 = cat.create_table(sch, TableSpec {
        name: "a".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "note".into(), data_type: DataType::Text, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        column_collations: Default::default(),
        column_checks: Default::default(),
    }).unwrap();
    let t2 = cat.create_table(sch, TableSpec {
        name: "b".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        column_collations: Default::default(),
        column_checks: Default::default(),
    }).unwrap();
    (cat, t1, t2)
}

#[test]
fn create_table_sets_version_to_one() {
    let (cat, t1, t2) = fresh_catalog();
    assert_eq!(cat.schema_version(t1), Some(1));
    assert_eq!(cat.schema_version(t2), Some(1));
}

#[test]
fn unknown_table_has_no_version() {
    let (cat, _, _) = fresh_catalog();
    assert_eq!(cat.schema_version(TableId(99_999)), None);
}

#[test]
fn rename_table_bumps_version() {
    let (mut cat, t1, _t2) = fresh_catalog();
    assert_eq!(cat.schema_version(t1), Some(1));
    cat.rename_table(t1, "a2").unwrap();
    assert_eq!(cat.schema_version(t1), Some(2));
}

#[test]
fn rename_column_bumps_version() {
    let (mut cat, t1, _t2) = fresh_catalog();
    cat.rename_column(t1, "note", "memo").unwrap();
    assert_eq!(cat.schema_version(t1), Some(2));
}

#[test]
fn drop_column_bumps_version() {
    let (mut cat, t1, _t2) = fresh_catalog();
    cat.drop_column(t1, "note").unwrap();
    assert_eq!(cat.schema_version(t1), Some(2));
}

#[test]
fn drop_table_removes_version() {
    let (mut cat, t1, _t2) = fresh_catalog();
    cat.drop_table(t1).unwrap();
    assert_eq!(cat.schema_version(t1), None);
}

#[test]
fn snapshot_reconciles_against_live_catalog() {
    let (cat, t1, t2) = fresh_catalog();
    let snap = cat.capture_schema_snapshot([t1, t2]);
    assert!(snap.reconcile(&cat).is_ok());
}

#[test]
fn snapshot_detects_bump_on_observed_table() {
    let (mut cat, t1, t2) = fresh_catalog();
    let snap = cat.capture_schema_snapshot([t1, t2]);
    cat.rename_table(t1, "renamed").unwrap();
    let err: SchemaStale = snap.reconcile(&cat).unwrap_err();
    assert_eq!(err.table, t1);
    assert_eq!(err.captured, 1);
    assert_eq!(err.current, 2);
}

#[test]
fn snapshot_ignores_unobserved_tables() {
    let (mut cat, t1, t2) = fresh_catalog();
    // Only observe t1; mutating t2 must not disturb reconcile.
    let snap = cat.capture_schema_snapshot([t1]);
    cat.rename_table(t2, "b2").unwrap();
    assert!(snap.reconcile(&cat).is_ok());
}

#[test]
fn snapshot_detects_dropped_observed_table() {
    let (mut cat, t1, _t2) = fresh_catalog();
    let snap = cat.capture_schema_snapshot([t1]);
    cat.drop_table(t1).unwrap();
    let err = snap.reconcile(&cat).unwrap_err();
    assert_eq!(err.table, t1);
    assert_eq!(err.captured, 1);
    assert_eq!(err.current, 0);
}

#[test]
fn full_snapshot_includes_every_table() {
    let (cat, t1, t2) = fresh_catalog();
    let snap: SchemaSnapshot = cat.capture_full_schema_snapshot();
    let entries: Vec<_> = snap.entries().collect();
    assert_eq!(entries.len(), 2);
    assert_eq!(snap.version_of(t1), Some(1));
    assert_eq!(snap.version_of(t2), Some(1));
}

#[test]
fn set_not_null_bumps_version() {
    let (mut cat, t1, _t2) = fresh_catalog();
    cat.strengthen_column_not_null(t1, "note").unwrap();
    assert_eq!(cat.schema_version(t1), Some(2));
}

#[test]
fn widen_bumps_version() {
    let (mut cat, _t1, _t2) = fresh_catalog();
    let db = cat.databases().next().unwrap().id;
    let sch = cat.schemas().find(|s| s.database == db).unwrap().id;
    let t3 = cat.create_table(sch, TableSpec {
        name: "c".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "amount".into(), data_type: DataType::Int32, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        column_collations: Default::default(),
        column_checks: Default::default(),
    }).unwrap();
    assert_eq!(cat.schema_version(t3), Some(1));
    cat.widen_column_type(t3, "amount", DataType::Int64).unwrap();
    assert_eq!(cat.schema_version(t3), Some(2));
}

#[test]
fn widen_noop_does_not_bump_version() {
    let (mut cat, t1, _t2) = fresh_catalog();
    // same-type widen is Ok(()) but a no-op: version must NOT bump.
    let before = cat.schema_version(t1).unwrap();
    cat.widen_column_type(t1, "note", DataType::Text).unwrap();
    assert_eq!(cat.schema_version(t1), Some(before));
}
