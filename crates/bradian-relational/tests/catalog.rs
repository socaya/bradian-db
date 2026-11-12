use bradian_relational::{
    Catalog, CatalogError, Column, DataType, ForeignKeySpec, PrimaryKey,
    TableSpec, UniqueKey,
};

fn column(name: &str, data_type: DataType, nullable: bool) -> Column {
    Column { name: name.into(), data_type, nullable }
}

fn spec(name: &str) -> TableSpec {
    TableSpec {
        name: name.into(),
        columns: vec![
            column("id", DataType::Int64, false),
            column("caption", DataType::Varchar { max_length: 120 }, true),
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }
}

#[test]
fn create_multiple_databases_and_schemas_with_scoped_names() {
    let mut cat = Catalog::new();
    let a = cat.create_database("Production").unwrap();
    let b = cat.create_database("analytics").unwrap();
    assert_ne!(a, b);
    assert!(matches!(cat.create_database("PRODUCTION"), Err(CatalogError::Duplicate(_))));
    let sa = cat.create_schema(a, "Public").unwrap();
    let sb = cat.create_schema(b, "public").unwrap();
    let t1 = cat.create_table(sa, spec("accounts")).unwrap();
    let t2 = cat.create_table(sb, spec("accounts")).unwrap();
    assert_ne!(t1, t2);
    assert_eq!(cat.database(a).unwrap().name, "production");
    assert_eq!(cat.schemas().count(), 2);
    assert_eq!(cat.tables().count(), 2);
}

#[test]
fn rejects_invalid_names_duplicate_columns_and_invalid_types() {
    let mut cat = Catalog::new();
    assert!(matches!(cat.create_database("../invalid"), Err(CatalogError::InvalidIdentifier(_))));
    let db = cat.create_database("test").unwrap();
    let schema = cat.create_schema(db, "public").unwrap();
    let mut duplicate = spec("records");
    duplicate.columns.push(column("ID", DataType::Int64, false));
    assert!(matches!(cat.create_table(schema, duplicate), Err(CatalogError::Duplicate(_))));
    let mut invalid = spec("records");
    invalid.columns[1].data_type = DataType::Decimal { precision: 2, scale: 5 };
    assert!(matches!(cat.create_table(schema, invalid), Err(CatalogError::InvalidDefinition(_))));
    let mut varchar = spec("records");
    varchar.columns[1].data_type = DataType::Varchar { max_length: 0 };
    assert!(matches!(cat.create_table(schema, varchar), Err(CatalogError::InvalidDefinition(_))));
}

#[test]
fn enforces_pk_nonnull_and_existing_key_columns() {
    let mut cat = Catalog::new();
    let db = cat.create_database("prod").unwrap();
    let s = cat.create_schema(db, "public").unwrap();
    let mut nullable = spec("invoice");
    nullable.columns[0].nullable = true;
    assert!(matches!(cat.create_table(s, nullable), Err(CatalogError::InvalidDefinition(_))));
    let mut missing = spec("invoice");
    missing.primary_key.columns = vec!["unknown".into()];
    assert!(matches!(cat.create_table(s, missing), Err(CatalogError::NotFound(_))));
    assert_eq!(cat.tables().count(), 0);
}

#[test]
fn foreign_key_restricts_drop_and_supports_cross_schema_same_db() {
    let mut cat = Catalog::new();
    let db = cat.create_database("prod").unwrap();
    let accounts_schema = cat.create_schema(db, "master").unwrap();
    let orders_schema = cat.create_schema(db, "sales").unwrap();
    let parent = cat.create_table(accounts_schema, spec("accounts")).unwrap();
    let mut child = spec("orders");
    child.columns.push(column("account_id", DataType::Int64, false));
    let orders = cat.create_table(orders_schema, child).unwrap();
    cat.add_foreign_key(orders, ForeignKeySpec {
        name: "fk_orders_accounts".into(),
        columns: vec!["account_id".into()],
        referenced_table: parent,
        referenced_columns: vec!["id".into()],
    }).unwrap();
    assert_eq!(cat.table(orders).unwrap().foreign_keys.len(), 1);
    assert!(matches!(cat.drop_table(parent), Err(CatalogError::DependentObjects(_))));
    cat.drop_table(orders).unwrap();
    cat.drop_table(parent).unwrap();
    cat.drop_schema(accounts_schema).unwrap();
    cat.drop_schema(orders_schema).unwrap();
    cat.drop_database(db).unwrap();
}

#[test]
fn foreign_keys_require_type_and_unique_target_match() {
    let mut cat = Catalog::new();
    let db = cat.create_database("prod").unwrap();
    let s = cat.create_schema(db, "public").unwrap();
    let parent = cat.create_table(s, spec("parent")).unwrap();
    let mut child = spec("child");
    child.columns.push(column("wrong_type", DataType::Text, true));
    let child_id = cat.create_table(s, child).unwrap();
    let err = cat.add_foreign_key(child_id, ForeignKeySpec {
        name: "bad_fk".into(), columns: vec!["wrong_type".into()],
        referenced_table: parent, referenced_columns: vec!["id".into()],
    }).unwrap_err();
    assert!(matches!(err, CatalogError::InvalidDefinition(_)));
    assert!(cat.table(child_id).unwrap().foreign_keys.is_empty());
}

#[test]
fn recognizes_nonnull_unique_keys_as_reference_targets() {
    let mut cat = Catalog::new();
    let db = cat.create_database("prod").unwrap();
    let s = cat.create_schema(db, "public").unwrap();
    let mut parent = spec("parent");
    parent.columns.push(column("external_id", DataType::Text, false));
    parent.unique_keys.push(UniqueKey {
        name: "uq_parent_external".into(),
        columns: vec!["external_id".into()],
    });
    let p = cat.create_table(s, parent).unwrap();
    let mut child = spec("child");
    child.columns.push(column("external_id", DataType::Text, true));
    let c = cat.create_table(s, child).unwrap();
    cat.add_foreign_key(c, ForeignKeySpec {
        name: "fk_child_parent_external".into(),
        columns: vec!["external_id".into()],
        referenced_table: p,
        referenced_columns: vec!["external_id".into()],
    }).unwrap();
}

#[test]
fn forbids_cross_database_foreign_keys() {
    let mut cat = Catalog::new();
    let d1 = cat.create_database("one").unwrap();
    let d2 = cat.create_database("two").unwrap();
    let s1 = cat.create_schema(d1, "public").unwrap();
    let s2 = cat.create_schema(d2, "public").unwrap();
    let p = cat.create_table(s1, spec("parent")).unwrap();
    let c = cat.create_table(s2, spec("child")).unwrap();
    assert!(matches!(cat.add_foreign_key(c, ForeignKeySpec {
        name: "fk".into(), columns: vec!["id".into()], referenced_table: p,
        referenced_columns: vec!["id".into()],
    }), Err(CatalogError::InvalidDefinition(_))));
}

#[test]
fn prevents_unsafe_drop_of_nonempty_schema_or_database() {
    let mut cat = Catalog::new();
    let db = cat.create_database("prod").unwrap();
    let schema = cat.create_schema(db, "public").unwrap();
    let table = cat.create_table(schema, spec("users")).unwrap();
    assert!(matches!(cat.drop_schema(schema), Err(CatalogError::DependentObjects(_))));
    assert!(matches!(cat.drop_database(db), Err(CatalogError::DependentObjects(_))));
    cat.drop_table(table).unwrap();
    cat.drop_schema(schema).unwrap();
    cat.drop_database(db).unwrap();
    assert_eq!(cat.databases().count(), 0);
}

#[test]
fn rejects_empty_and_duplicate_key_components() {
    let mut cat = Catalog::new();
    let db = cat.create_database("prod").unwrap();
    let schema = cat.create_schema(db, "public").unwrap();
    let mut empty = spec("record");
    empty.primary_key.columns.clear();
    assert!(matches!(cat.create_table(schema, empty), Err(CatalogError::InvalidDefinition(_))));
    let mut repeated = spec("record");
    repeated.primary_key.columns = vec!["id".into(), "ID".into()];
    assert!(matches!(cat.create_table(schema, repeated), Err(CatalogError::Duplicate(_))));
}
