//! ADR 0157 — catalog storage for row/column/tenant
//! policies. Programmatic API only; `CREATE POLICY`
//! grammar is a follow-up.

use bradian_relational::access_policy::{
    ColumnLevelPolicy, RowLevelPolicy, RowPredicate, TableAccessPolicy, TenantIsolation,
};
use bradian_relational::{Catalog, Column, DataType, PrimaryKey, TableSpec};
use bradian_relational::typed_table::Value;

fn make_table() -> (Catalog, bradian_relational::TableId) {
    let mut c = Catalog::new();
    let db = c.create_database("app").unwrap();
    let sch = c.create_schema(db, "public").unwrap();
    let t = c.create_table(sch, TableSpec {
        name: "items".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "tenant".into(), data_type: DataType::Text, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
    }).unwrap();
    (c, t)
}

fn example_policy() -> TableAccessPolicy {
    TableAccessPolicy {
        row_level: RowLevelPolicy {
            predicate: RowPredicate::ColumnEquals {
                column: "tenant".into(),
                value: Value::String("acme".into()),
            },
        },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: Some(TenantIsolation::single_tenant("tenant", "acme")),
    }
}

#[test]
fn set_policy_and_get_roundtrips() {
    let (mut c, t) = make_table();
    c.set_policy(t, "acme_only", example_policy()).unwrap();
    let got = c.get_policy(t, "acme_only").unwrap();
    assert_eq!(got, &example_policy());
}

#[test]
fn drop_policy_removes_it() {
    let (mut c, t) = make_table();
    c.set_policy(t, "acme_only", example_policy()).unwrap();
    assert!(c.get_policy(t, "acme_only").is_some());
    c.drop_policy(t, "acme_only").unwrap();
    assert!(c.get_policy(t, "acme_only").is_none());
}

#[test]
fn set_policy_overwrites_existing_name() {
    let (mut c, t) = make_table();
    let p1 = example_policy();
    let p2 = TableAccessPolicy {
        row_level: RowLevelPolicy { predicate: RowPredicate::AlwaysDeny },
        column_level: ColumnLevelPolicy::deny_all(),
        tenant: None,
    };
    c.set_policy(t, "p", p1).unwrap();
    c.set_policy(t, "p", p2.clone()).unwrap();
    assert_eq!(c.get_policy(t, "p").unwrap(), &p2);
}

#[test]
fn policies_for_table_lists_every_attached_policy() {
    let (mut c, t) = make_table();
    c.set_policy(t, "a", example_policy()).unwrap();
    c.set_policy(t, "b", example_policy()).unwrap();
    let all = c.policies_for_table(t).unwrap();
    assert_eq!(all.len(), 2);
    assert!(all.contains_key("a"));
    assert!(all.contains_key("b"));
}

#[test]
fn set_policy_unknown_table_errors() {
    let mut c = Catalog::new();
    let err = c.set_policy(
        bradian_relational::TableId(999), "p", example_policy()).unwrap_err();
    assert!(matches!(err, bradian_relational::CatalogError::NotFound(_)));
}

#[test]
fn drop_policy_on_unknown_name_is_no_op() {
    let (mut c, t) = make_table();
    c.drop_policy(t, "never_set").unwrap();
}

#[test]
fn table_without_policies_has_empty_map() {
    let (c, t) = make_table();
    assert!(c.policies_for_table(t).unwrap().is_empty());
}

#[test]
fn policy_round_trips_through_serde_json() {
    let p = example_policy();
    let json = serde_json::to_string(&p).unwrap();
    let back: TableAccessPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(p, back);
}

#[test]
fn legacy_table_without_policies_field_still_deserializes() {
    // ADR 0157 — the policies map has #[serde(default,
    // skip_serializing_if = "is_empty")], so pre-ADR-0157
    // schema JSON (without the policies key) deserializes
    // cleanly to an empty map.
    let legacy = r#"{
        "id": 8,
        "schema": 2,
        "name": "entries",
        "columns": [{"name":"id","data_type":"Int64","nullable":false}],
        "primary_key": {"columns":["id"]},
        "unique_keys": [],
        "foreign_keys": []
    }"#;
    let table: bradian_relational::Table = serde_json::from_str(legacy).unwrap();
    assert!(table.policies.is_empty());
}
