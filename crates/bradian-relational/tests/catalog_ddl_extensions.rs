//! ADR 0124 — in-memory Catalog DDL extensions: RENAME TABLE,
//! RENAME COLUMN, DROP COLUMN, SET/DROP DEFAULT.
//!
//! These exercise the Catalog mutation primitives only;
//! PersistentCatalog journaling, SQL-parser wiring and
//! HistorySqlEngine schema-reconciliation remain OPEN.

use bradian_relational::{
    Catalog, CatalogError, Column, DataType, ForeignKeySpec, PrimaryKey,
    TableId, TableSpec, UniqueKey,
    typed_table::Value,
};

fn mk_catalog() -> (Catalog, TableId) {
    let mut c = Catalog::new();
    let db = c.create_database("app").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let t = c.create_table(sch, TableSpec {
        name: "items".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "sku".into(), data_type: DataType::Text, nullable: false },
            Column { name: "qty".into(), data_type: DataType::Int32, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![UniqueKey {
            name: "items_sku_key".into(),
            columns: vec!["sku".into()],
        }],
        ..Default::default()
    }).unwrap();
    (c, t)
}

#[test]
fn rename_table_updates_name() {
    let (mut c, t) = mk_catalog();
    c.rename_table(t, "widgets").unwrap();
    assert_eq!(c.table(t).unwrap().name, "widgets");
}

#[test]
fn rename_table_rejects_duplicate() {
    let (mut c, t) = mk_catalog();
    let sch = c.table(t).unwrap().schema;
    c.create_table(sch, TableSpec {
        name: "other".into(),
        columns: vec![Column { name: "a".into(), data_type: DataType::Int32, nullable: false }],
        primary_key: PrimaryKey { columns: vec!["a".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    let err = c.rename_table(t, "other").unwrap_err();
    assert!(matches!(err, CatalogError::Duplicate(_)));
}

#[test]
fn rename_column_updates_column_pk_and_uk() {
    let (mut c, t) = mk_catalog();
    c.rename_column(t, "sku", "code").unwrap();
    let tbl = c.table(t).unwrap();
    assert!(tbl.columns.iter().any(|c| c.name == "code"));
    assert!(!tbl.columns.iter().any(|c| c.name == "sku"));
    assert_eq!(tbl.unique_keys[0].columns, vec!["code".to_string()]);
}

#[test]
fn rename_column_rejects_duplicate() {
    let (mut c, t) = mk_catalog();
    let err = c.rename_column(t, "sku", "qty").unwrap_err();
    assert!(matches!(err, CatalogError::Duplicate(_)));
}

#[test]
fn rename_column_unknown_errors() {
    let (mut c, t) = mk_catalog();
    assert!(matches!(c.rename_column(t, "zzz", "yyy"), Err(CatalogError::NotFound(_))));
}

#[test]
fn rename_column_updates_incoming_fk() {
    let (mut c, t) = mk_catalog();
    let sch = c.table(t).unwrap().schema;
    let child = c.create_table(sch, TableSpec {
        name: "lines".into(),
        columns: vec![
            Column { name: "lid".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "item_id".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["lid".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    c.add_foreign_key(child, ForeignKeySpec {
        name: "fk_item".into(),
        columns: vec!["item_id".into()],
        referenced_table: t,
        referenced_columns: vec!["id".into()],
    }).unwrap();
    c.rename_column(t, "id", "item_pk").unwrap();
    let fk = &c.table(child).unwrap().foreign_keys[0];
    assert_eq!(fk.referenced_columns, vec!["item_pk".to_string()]);
}

#[test]
fn drop_column_removes_non_key_column() {
    let (mut c, t) = mk_catalog();
    c.drop_column(t, "qty").unwrap();
    assert!(!c.table(t).unwrap().columns.iter().any(|c| c.name == "qty"));
}

#[test]
fn drop_column_rejects_pk_column() {
    let (mut c, t) = mk_catalog();
    let err = c.drop_column(t, "id").unwrap_err();
    assert!(matches!(err, CatalogError::DependentObjects(_)));
}

#[test]
fn drop_column_rejects_uk_column() {
    let (mut c, t) = mk_catalog();
    let err = c.drop_column(t, "sku").unwrap_err();
    assert!(matches!(err, CatalogError::DependentObjects(_)));
}

#[test]
fn drop_column_rejects_last_column() {
    let mut c = Catalog::new();
    let db = c.create_database("app").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let t = c.create_table(sch, TableSpec {
        name: "t".into(),
        columns: vec![Column { name: "only".into(), data_type: DataType::Int32, nullable: false }],
        primary_key: PrimaryKey { columns: vec!["only".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    let err = c.drop_column(t, "only").unwrap_err();
    // Last-column check or PK-check — either is acceptable.
    assert!(matches!(err, CatalogError::InvalidDefinition(_) | CatalogError::DependentObjects(_)));
}

#[test]
fn drop_column_rejects_incoming_fk_referenced() {
    let (mut c, t) = mk_catalog();
    let sch = c.table(t).unwrap().schema;
    let child = c.create_table(sch, TableSpec {
        name: "lines".into(),
        columns: vec![
            Column { name: "lid".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "item_sku".into(), data_type: DataType::Text, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["lid".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    c.add_foreign_key(child, ForeignKeySpec {
        name: "fk_sku".into(),
        columns: vec!["item_sku".into()],
        referenced_table: t,
        referenced_columns: vec!["sku".into()],
    }).unwrap();
    let err = c.drop_column(t, "sku").unwrap_err();
    assert!(matches!(err, CatalogError::DependentObjects(_)));
}

#[test]
fn set_column_default_validates_type() {
    let (mut c, t) = mk_catalog();
    c.set_column_default(t, "qty", Value::Int32(5)).unwrap();
    assert_eq!(c.table(t).unwrap().column_defaults.get("qty"), Some(&Value::Int32(5)));
}

#[test]
fn set_column_default_rejects_type_mismatch() {
    let (mut c, t) = mk_catalog();
    let err = c.set_column_default(t, "qty", Value::String("five".into())).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn set_column_default_rejects_null() {
    let (mut c, t) = mk_catalog();
    let err = c.set_column_default(t, "qty", Value::Null).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn set_column_default_rejects_unknown_column() {
    let (mut c, t) = mk_catalog();
    let err = c.set_column_default(t, "nope", Value::Int32(0)).unwrap_err();
    assert!(matches!(err, CatalogError::NotFound(_)));
}

#[test]
fn drop_column_default_is_idempotent() {
    let (mut c, t) = mk_catalog();
    c.drop_column_default(t, "qty").unwrap();   // no default set yet
    c.set_column_default(t, "qty", Value::Int32(5)).unwrap();
    c.drop_column_default(t, "qty").unwrap();
    assert!(c.table(t).unwrap().column_defaults.is_empty());
}

#[test]
fn drop_column_default_rejects_unknown_column() {
    let (mut c, t) = mk_catalog();
    let err = c.drop_column_default(t, "nope").unwrap_err();
    assert!(matches!(err, CatalogError::NotFound(_)));
}
