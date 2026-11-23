//! ADR 0232 — scalar functions in SELECT projection.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, nickname TEXT, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, nickname, qty) VALUES(1, 'Alpha', NULL, 10), (2, 'Bravo', 'B', -5), (3, 'Charlie', NULL, 0)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> (Vec<String>, Vec<Vec<Value>>) {
    match rs {
        SqlResultSet::Rows { columns, rows } => (columns, rows),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn upper_lowers_single_column() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute("SELECT UPPER(name), LOWER(name) FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(cols, vec!["upper", "lower"]);
    assert_eq!(rows, vec![vec![Value::String("ALPHA".into()), Value::String("alpha".into())]]);
}

#[test]
fn length_counts_chars() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT LENGTH(name) FROM t.public.items WHERE id = 3").unwrap());
    assert_eq!(rows, vec![vec![Value::Int64(7)]]);
}

#[test]
fn abs_on_negative() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT ABS(qty) FROM t.public.items WHERE id = 2").unwrap());
    assert_eq!(rows, vec![vec![Value::Int64(5)]]);
}

#[test]
fn coalesce_returns_first_non_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT COALESCE(nickname, name) FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(rows, vec![vec![Value::String("Alpha".into())]]);
    let (_, rows) = rows_of(db.execute("SELECT COALESCE(nickname, name) FROM t.public.items WHERE id = 2").unwrap());
    assert_eq!(rows, vec![vec![Value::String("B".into())]]);
}

#[test]
fn coalesce_with_trailing_literal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT COALESCE(nickname, 'default') FROM t.public.items WHERE id = 3").unwrap());
    assert_eq!(rows, vec![vec![Value::String("default".into())]]);
}

#[test]
fn nullif_returns_null_on_equal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT NULLIF(qty, 0) FROM t.public.items WHERE id = 3").unwrap());
    assert_eq!(rows, vec![vec![Value::Null]]);
}

#[test]
fn nullif_returns_value_on_unequal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT NULLIF(qty, 0) FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(rows, vec![vec![Value::Int64(10)]]);
}

#[test]
fn null_propagates_through_upper() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute("SELECT UPPER(nickname) FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(rows, vec![vec![Value::Null]]);
}

#[test]
fn upper_on_numeric_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT UPPER(qty) FROM t.public.items WHERE id = 1").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn mixed_projection_with_bare_column_and_func() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute("SELECT id, UPPER(name), LENGTH(name) FROM t.public.items WHERE id = 1").unwrap());
    assert_eq!(cols, vec!["id", "upper", "length"]);
    assert_eq!(rows, vec![vec![
        Value::Int64(1), Value::String("ALPHA".into()), Value::Int64(5),
    ]]);
}

#[test]
fn abs_on_text_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT ABS(name) FROM t.public.items WHERE id = 1").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}
