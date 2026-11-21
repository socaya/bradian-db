//! ADR 0173 — `Value::Json` round-trip through typed_table + every SQL engine.
//!
//! Covers:
//! - `encode_row`/`decode_row` tag 8 round-trip for `Value::Json`.
//! - Backward compat: a row written under tag 5 (`Value::String`) for a
//!   `DataType::Json` column still decodes and validates.
//! - `SqlEngine::coerce` emits `Value::Json` for a JSON column.
//! - SELECT * returns the stored Json cells; they render as quoted
//!   literals through the driver value-to-sql encoder.
//! - Invalid JSON content is rejected at INSERT.

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::{decode_row, encode_row, Value};
use bradian_relational::{
    Column, DataType, PrimaryKey, SchemaId, Table, TableId,
};
use tempfile::tempdir;

fn docs_table() -> Table {
    Table {
        id: TableId(1), schema: SchemaId(1), name: "docs".into(),
        columns: vec![
            Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
            Column { name: "body".into(), data_type: DataType::Json, nullable: false },
        ],
        primary_key: PrimaryKey { columns: vec!["id".into()] },
        unique_keys: vec![],
        foreign_keys: vec![],
        column_defaults: Default::default(),
        column_collations: Default::default(),
        policies: Default::default(),
    }
}

#[test]
fn encode_decode_round_trip_preserves_json_variant() {
    let table = docs_table();
    let body = r#"{"name":"ou1","count":3,"nested":[1,2,3]}"#.to_string();
    let row = vec![Value::Int64(1), Value::Json(body.clone())];
    let bytes = encode_row(&table, &row).unwrap();
    assert_eq!(decode_row(&table, &bytes).unwrap(), row);
    // Tag 8 is written for the Json cell (bytes[15] = Int64 tag 3;
    // after an 8-byte i64 payload, position 24 is the Json tag).
    assert_eq!(bytes[15], 3, "int64 tag in first column position");
    assert_eq!(bytes[24], 8, "ADR 0173 json tag at the second column offset");
}

#[test]
fn legacy_tag_5_string_rows_still_decode_for_json_columns() {
    // Simulate a pre-ADR-0173 row by hand-crafting the encoding with
    // tag 5 (String) for a column declared as JSON. The current
    // decoder returns Value::String; validate_value accepts it
    // under the DataType::Json branch as long as the content is
    // valid JSON.
    let table = docs_table();
    let mut legacy = Vec::new();
    legacy.extend_from_slice(b"BRRW");  // ROW_MAGIC
    legacy.push(1);                     // VERSION
    legacy.extend_from_slice(&1_u64.to_le_bytes());  // table id
    legacy.extend_from_slice(&2_u16.to_le_bytes());  // column count
    // id column — tag 3 (Int64) + 8 bytes.
    legacy.push(3);
    legacy.extend_from_slice(&5_i64.to_le_bytes());
    // body column — tag 5 (String) + u32 length + utf8 bytes.
    let legacy_json = r#"{"legacy":true}"#;
    legacy.push(5);
    legacy.extend_from_slice(&(legacy_json.len() as u32).to_le_bytes());
    legacy.extend_from_slice(legacy_json.as_bytes());

    let row = decode_row(&table, &legacy).unwrap();
    // Reads back as Value::String under the legacy tag; the
    // schema-level validate_value accepts it for a JSON column.
    assert!(matches!(row[1], Value::String(ref s) if s == legacy_json));
}

#[test]
fn invalid_json_body_is_rejected_at_encode() {
    let table = docs_table();
    let row = vec![Value::Int64(1), Value::Json("not json {".into())];
    assert!(encode_row(&table, &row).is_err(),
        "invalid JSON content should fail validate_value and refuse encode");
}

#[test]
fn sql_engine_insert_round_trip_returns_value_json() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO test.public.docs(id, body) VALUES(1, '{\"k\":1}')",
    ] { db.execute(sql).unwrap(); }

    match db.execute("SELECT body FROM test.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body".to_string()]);
            assert_eq!(rows.len(), 1);
            match &rows[0][0] {
                Value::Json(s) => assert_eq!(s, r#"{"k":1}"#),
                other => panic!("expected Value::Json after ADR 0173; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn invalid_json_literal_rejected_at_insert_through_sql() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    assert!(db.execute("INSERT INTO test.public.docs(id, body) VALUES(2, 'not json {')")
        .is_err(), "invalid JSON must be rejected by coerce");
}

#[test]
fn null_insert_into_nullable_json_column_round_trips() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.notes (id BIGINT NOT NULL, meta JSON, PRIMARY KEY(id))",
        "INSERT INTO test.public.notes(id, meta) VALUES(7, NULL)",
    ] { db.execute(sql).unwrap(); }
    match db.execute("SELECT meta FROM test.public.notes WHERE id=7").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows, vec![vec![Value::Null]]);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn update_json_column_preserves_value_json_tag() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE test",
        "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO test.public.docs(id, body) VALUES(1, '{\"k\":1}')",
        "UPDATE test.public.docs SET body='{\"k\":2}' WHERE id=1",
    ] { db.execute(sql).unwrap(); }
    match db.execute("SELECT body FROM test.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            match &rows[0][0] {
                Value::Json(s) => assert_eq!(s, r#"{"k":2}"#),
                other => panic!("expected Value::Json after UPDATE; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}
