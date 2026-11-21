//! ADR 0126 — Dependency planner: pure analysis that lists
//! every catalog dependency edge that blocks a DDL. No
//! mutation. Complements the single-reason refusals in
//! existing drop_table / drop_column paths.

use bradian_relational::{
    Catalog, Column, DataType, Dependency, ForeignKeySpec, PrimaryKey,
    TableId, TableSpec, UniqueKey,
};

fn mk() -> (Catalog, TableId, TableId) {
    let mut c = Catalog::new();
    let db = c.create_database("app").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let parent = c.create_table(sch, TableSpec {
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
    let child = c.create_table(sch, TableSpec {
        name: "lines".into(),
        columns: vec![
            Column { name: "lid".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "item_id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "item_sku".into(), data_type: DataType::Text, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["lid".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    c.add_foreign_key(child, ForeignKeySpec {
        name: "fk_item_id".into(),
        columns: vec!["item_id".into()],
        referenced_table: parent,
        referenced_columns: vec!["id".into()],
    }).unwrap();
    c.add_foreign_key(child, ForeignKeySpec {
        name: "fk_item_sku".into(),
        columns: vec!["item_sku".into()],
        referenced_table: parent,
        referenced_columns: vec!["sku".into()],
    }).unwrap();
    (c, parent, child)
}

#[test]
fn table_dependents_lists_every_incoming_fk() {
    let (c, parent, child) = mk();
    let deps = c.table_dependents(parent);
    assert_eq!(deps.len(), 2);
    for d in &deps {
        match d {
            Dependency::IncomingForeignKey { from_table, .. } => {
                assert_eq!(*from_table, child);
            }
            other => panic!("expected IncomingFK, got {other:?}"),
        }
    }
}

#[test]
fn table_dependents_empty_for_leaf_table() {
    let (c, _, child) = mk();
    assert!(c.table_dependents(child).is_empty());
}

#[test]
fn table_dependents_unknown_table_returns_empty() {
    let c = Catalog::new();
    assert!(c.table_dependents(TableId(9999)).is_empty());
}

#[test]
fn column_dependents_lists_pk_uk_and_incoming_fk() {
    let (c, parent, _) = mk();
    // Column `id` is PK + referenced by an incoming FK.
    let deps = c.column_dependents(parent, "id");
    assert!(deps.iter().any(|d| matches!(d, Dependency::PrimaryKey { .. })));
    assert!(deps.iter().any(|d| matches!(d, Dependency::IncomingForeignKey { .. })));
    assert_eq!(deps.len(), 2);

    // Column `sku` is UK + referenced by an incoming FK.
    let deps = c.column_dependents(parent, "sku");
    assert!(deps.iter().any(|d| matches!(d,
        Dependency::UniqueKey { constraint, .. } if constraint == "items_sku_key")));
    assert!(deps.iter().any(|d| matches!(d,
        Dependency::IncomingForeignKey { constraint, .. } if constraint == "fk_item_sku")));
}

#[test]
fn column_dependents_lists_local_fk() {
    let (c, _, child) = mk();
    let deps = c.column_dependents(child, "item_id");
    assert!(deps.iter().any(|d| matches!(d,
        Dependency::ForeignKey { constraint, .. } if constraint == "fk_item_id")));
}

#[test]
fn column_dependents_empty_for_undepended_column() {
    let (c, parent, _) = mk();
    // qty is nullable, not PK/UK/FK/incoming-FK.
    assert!(c.column_dependents(parent, "qty").is_empty());
}

#[test]
fn column_dependents_unknown_column_returns_empty() {
    let (c, parent, _) = mk();
    assert!(c.column_dependents(parent, "nope").is_empty());
}

#[test]
fn column_dependents_reports_last_column_only() {
    let mut c = Catalog::new();
    let db = c.create_database("x").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let t = c.create_table(sch, TableSpec {
        name: "t".into(),
        columns: vec![Column { name: "only".into(), data_type: DataType::Int32, nullable: false }],
        primary_key: PrimaryKey { columns: vec!["only".into()] },
        unique_keys: vec![],
        ..Default::default()
    }).unwrap();
    let deps = c.column_dependents(t, "only");
    assert!(deps.iter().any(|d| matches!(d, Dependency::LastColumn)));
    assert!(deps.iter().any(|d| matches!(d, Dependency::PrimaryKey { .. })));
}

#[test]
fn column_dependents_is_pure() {
    // Running it many times must not mutate the catalog.
    let (c, parent, _) = mk();
    let before = c.tables().cloned().collect::<Vec<_>>();
    for _ in 0..10 {
        let _ = c.column_dependents(parent, "id");
        let _ = c.column_dependents(parent, "sku");
        let _ = c.table_dependents(parent);
    }
    let after = c.tables().cloned().collect::<Vec<_>>();
    assert_eq!(before, after);
}
