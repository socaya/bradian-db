//! ADR 0191 — JSON array index segments in path.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["red","green","blue"]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tags":[{"c":"x"},{"c":"y"}]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"tags":[]}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn arrow_arrow_array_index_returns_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' ->> 1 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body->'tags'->>1".to_string()]);
            match &rows[0][0] {
                Value::String(s) => assert_eq!(s, "green"),
                other => panic!("expected String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_array_index_returns_json() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' -> 0 FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => {
                let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                assert_eq!(parsed["c"], "x");
            }
            other => panic!("expected Json; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn chained_index_and_key_in_one_path() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' -> 1 ->> 'c' FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "y"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn out_of_bounds_index_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' -> 99 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn empty_array_index_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' -> 0 FROM t.public.docs WHERE id=3").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_filter_by_array_index_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT id FROM t.public.docs WHERE body -> 'tags' ->> 0 = 'red'").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(1));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn negative_index_resolves_from_end_after_adr_0196() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ADR 0196 reverses the ADR 0191 negative-rejection: `-1`
    // now resolves to the last element. The existing tests
    // continue to use non-negative indices.
    match db.execute("SELECT body -> 'tags' ->> -1 FROM t.public.docs WHERE id=1").unwrap() {
        bradian_relational::sql::SqlResultSet::Rows { rows, .. } => {
            match &rows[0][0] {
                Value::String(s) => assert_eq!(s, "blue"),
                other => panic!("expected String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn float_index_is_rejected_at_parse() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body -> 'tags' -> 1.5 FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn applying_index_to_object_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // body is an object; `body -> 0` requests index 0 of an
    // object → missing → NULL.
    match db.execute("SELECT body -> 0 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}
