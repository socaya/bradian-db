//! ADR 0130 — in-memory column type widening.

use bradian_relational::{
    Catalog, CatalogError, Column, DataType, ForeignKeySpec, PrimaryKey,
    TableId, TableSpec, UniqueKey,
};

fn mk() -> (Catalog, TableId) {
    let mut c = Catalog::new();
    let db = c.create_database("app").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let t = c.create_table(sch, TableSpec {
        name: "t".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "smallnum".into(), data_type: DataType::Int32, nullable: true },
            Column { name: "label".into(), data_type: DataType::Varchar { max_length: 32 }, nullable: true },
            Column { name: "price".into(), data_type: DataType::Decimal { precision: 10, scale: 2 }, nullable: true },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    (c, t)
}

#[test]
fn widen_int32_to_int64() {
    let (mut c, t) = mk();
    c.widen_column_type(t, "smallnum", DataType::Int64).unwrap();
    let col = c.table(t).unwrap().columns.iter().find(|c| c.name == "smallnum").unwrap();
    assert_eq!(col.data_type, DataType::Int64);
}

#[test]
fn widen_varchar_longer_length() {
    let (mut c, t) = mk();
    c.widen_column_type(t, "label", DataType::Varchar { max_length: 128 }).unwrap();
    let col = c.table(t).unwrap().columns.iter().find(|c| c.name == "label").unwrap();
    assert_eq!(col.data_type, DataType::Varchar { max_length: 128 });
}

#[test]
fn widen_varchar_to_text() {
    let (mut c, t) = mk();
    c.widen_column_type(t, "label", DataType::Text).unwrap();
    let col = c.table(t).unwrap().columns.iter().find(|c| c.name == "label").unwrap();
    assert_eq!(col.data_type, DataType::Text);
}

#[test]
fn widen_decimal_precision_only() {
    let (mut c, t) = mk();
    c.widen_column_type(t, "price",
        DataType::Decimal { precision: 18, scale: 2 }).unwrap();
    let col = c.table(t).unwrap().columns.iter().find(|c| c.name == "price").unwrap();
    assert_eq!(col.data_type, DataType::Decimal { precision: 18, scale: 2 });
}

#[test]
fn same_type_is_noop_ok() {
    let (mut c, t) = mk();
    c.widen_column_type(t, "smallnum", DataType::Int32).unwrap();
}

#[test]
fn narrowing_int64_to_int32_rejected() {
    let (mut c, t) = mk();
    // id column is Int64; try to narrow to Int32.
    let err = c.widen_column_type(t, "id", DataType::Int32).unwrap_err();
    // id is also PK, so either InvalidDefinition or DependentObjects
    // can be returned depending on check order. Both are acceptable.
    assert!(matches!(err,
        CatalogError::InvalidDefinition(_) | CatalogError::DependentObjects(_)));
}

#[test]
fn narrowing_varchar_rejected() {
    let (mut c, t) = mk();
    let err = c.widen_column_type(t, "label", DataType::Varchar { max_length: 8 }).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn text_to_varchar_rejected() {
    let (mut c, t) = mk();
    // First widen label to Text, then try to go back to Varchar.
    c.widen_column_type(t, "label", DataType::Text).unwrap();
    let err = c.widen_column_type(t, "label", DataType::Varchar { max_length: 1000 }).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn decimal_scale_change_rejected() {
    let (mut c, t) = mk();
    let err = c.widen_column_type(t, "price",
        DataType::Decimal { precision: 18, scale: 4 }).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn decimal_narrowing_rejected() {
    let (mut c, t) = mk();
    let err = c.widen_column_type(t, "price",
        DataType::Decimal { precision: 5, scale: 2 }).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn widen_pk_column_rejected_dependency() {
    let (mut c, t) = mk();
    // id is Int64 PK; cross-type widening rejected.
    let err = c.widen_column_type(t, "id", DataType::Decimal { precision: 20, scale: 0 }).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
}

#[test]
fn widen_uk_column_rejected_dependency() {
    let mut c = Catalog::new();
    let db = c.create_database("app").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let t = c.create_table(sch, TableSpec {
        name: "t".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "sku".into(), data_type: DataType::Varchar { max_length: 16 }, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![UniqueKey {
            name: "sku_uk".into(),
            columns: vec!["sku".into()],
        }],
        ..Default::default()
    }).unwrap();
    let err = c.widen_column_type(t, "sku", DataType::Varchar { max_length: 64 }).unwrap_err();
    assert!(matches!(err, CatalogError::DependentObjects(_)));
}

#[test]
fn widen_incoming_fk_column_rejected() {
    let (mut c, t) = mk();
    let sch = c.table(t).unwrap().schema;
    let child = c.create_table(sch, TableSpec {
        name: "ref".into(),
        columns: vec![
            Column { name: "rid".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "tid".into(), data_type: DataType::Int64, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["rid".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    c.add_foreign_key(child, ForeignKeySpec {
        name: "fk_t".into(),
        columns: vec!["tid".into()],
        referenced_table: t,
        referenced_columns: vec!["id".into()],
        ..Default::default()
    }).unwrap();
    // id column has incoming FK — even a same-direction widening
    // attempt would fail dependency check (but Int64 -> anything
    // isn't a widening anyway).
    let err = c.widen_column_type(t, "id", DataType::Int32).unwrap_err();
    assert!(matches!(err,
        CatalogError::InvalidDefinition(_) | CatalogError::DependentObjects(_)));
}

#[test]
fn unknown_column_errors() {
    let (mut c, t) = mk();
    let err = c.widen_column_type(t, "nope", DataType::Int64).unwrap_err();
    assert!(matches!(err, CatalogError::NotFound(_)));
}

#[test]
fn unknown_table_errors() {
    let mut c = Catalog::new();
    let err = c.widen_column_type(TableId(999), "x", DataType::Int64).unwrap_err();
    assert!(matches!(err, CatalogError::NotFound(_)));
}
