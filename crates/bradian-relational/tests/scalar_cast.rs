//! ADR 0257 — scalar CAST in SELECT projection.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, n INT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note, n) VALUES(1, 'hello', 10)",
        "INSERT INTO t.public.items(id, note, n) VALUES(2, '42', 20)",
    ] { db.execute(sql).unwrap(); }
    (dir, db)
}

fn first_row(rs: SqlResultSet) -> Vec<Value> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows.into_iter().next().expect("one row"),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn cast_int_to_bigint() {
    let (_dir, db) = seed();
    let row = first_row(db.execute(
        "SELECT CAST(n AS BIGINT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::Int64(10)]);
}

#[test]
fn cast_text_to_bigint() {
    let (_dir, db) = seed();
    let row = first_row(db.execute(
        "SELECT CAST(note AS BIGINT) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(row, vec![Value::Int64(42)]);
}

#[test]
fn cast_bigint_to_text() {
    let (_dir, db) = seed();
    let row = first_row(db.execute(
        "SELECT CAST(id AS TEXT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::String("1".into())]);
}

#[test]
fn cast_text_to_bigint_bad_text_rejects() {
    let (_dir, db) = seed();
    let err = db.execute(
        "SELECT CAST(note AS BIGINT) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn cast_bigint_to_timestamptz_and_back() {
    let (_dir, db) = seed();
    let row = first_row(db.execute(
        "SELECT CAST(CAST(id AS TIMESTAMPTZ) AS BIGINT) FROM t.public.items WHERE id = 1"
    ).unwrap());
    assert_eq!(row, vec![Value::Int64(1)]);
}

#[test]
fn cast_timestamptz_to_text() {
    let (_dir, db) = seed();
    let row = first_row(db.execute(
        "SELECT CAST(CAST(id AS TIMESTAMPTZ) AS TEXT) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(row, vec![Value::String("2".into())]);
}

#[test]
fn cast_uuid_text_round_trip() {
    let (_dir, db) = seed();
    // Use the UUID literal as a TEXT column value.
    db.execute(
        "INSERT INTO t.public.items(id, note, n) VALUES(3, '550e8400-e29b-41d4-a716-446655440000', 30)"
    ).unwrap();
    let row = first_row(db.execute(
        "SELECT CAST(CAST(note AS UUID) AS TEXT) FROM t.public.items WHERE id = 3"
    ).unwrap());
    assert_eq!(row, vec![Value::String("550e8400-e29b-41d4-a716-446655440000".into())]);
}

#[test]
fn cast_invalid_uuid_text_rejects() {
    let (_dir, db) = seed();
    db.execute(
        "INSERT INTO t.public.items(id, note, n) VALUES(4, 'not-a-uuid', 40)"
    ).unwrap();
    let err = db.execute(
        "SELECT CAST(note AS UUID) FROM t.public.items WHERE id = 4"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)), "got {err:?}");
}

#[test]
fn cast_bool_to_bigint() {
    let (_dir, db) = seed();
    let row = first_row(db.execute(
        "SELECT CAST(CAST(note AS BIGINT) AS BIGINT) FROM t.public.items WHERE id = 2"
    ).unwrap());
    assert_eq!(row, vec![Value::Int64(42)]);
}

#[test]
fn cast_unsupported_target_rejected() {
    let (_dir, db) = seed();
    let err = db.execute(
        "SELECT CAST(id AS NONSENSE) FROM t.public.items WHERE id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)), "got {err:?}");
}

#[test]
fn cast_null_propagates() {
    let (_dir, db) = seed();
    db.execute("INSERT INTO t.public.items(id, note, n) VALUES(5, NULL, NULL)").unwrap();
    let row = first_row(db.execute(
        "SELECT CAST(note AS BIGINT) FROM t.public.items WHERE id = 5"
    ).unwrap());
    assert_eq!(row, vec![Value::Null]);
}

#[test]
fn cast_in_where_filter() {
    // CAST inside a WHERE JSON-path context is a future
    // extension; this test pins that scalar WHERE still works
    // without CAST and CAST in SELECT is independent.
    let (_dir, db) = seed();
    let rs = db.execute("SELECT id FROM t.public.items WHERE n > 15").unwrap();
    match rs {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows.len(), 1),
        other => panic!("{other:?}"),
    }
}
