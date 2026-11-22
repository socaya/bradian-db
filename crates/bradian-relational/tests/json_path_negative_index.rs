//! ADR 0196 — negative JSON array indices (from-end).

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tags":[]}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn arrow_negative_one_returns_last_element() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' ->> -1 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "blue"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_negative_two_returns_second_to_last() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' ->> -2 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "green"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_negative_out_of_bounds_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // -4 on a length-3 array is out of bounds.
    match db.execute("SELECT body -> 'tags' -> -4 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_negative_on_empty_array_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'tags' -> -1 FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_path_accepts_negative_index() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body #>> '{tags,-1}' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "blue"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_filter_by_negative_index() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'tags' ->> -1 = 'blue'"
    ).unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(1));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn update_set_negative_index_replaces_last_element() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> -1 = 'yellow' WHERE id=1").unwrap();
    match db.execute("SELECT body -> 'tags' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => {
                let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                assert_eq!(parsed, serde_json::json!(["red", "green", "yellow"]));
            }
            other => panic!("expected Json; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn update_set_negative_index_out_of_bounds_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> -99 = 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)),
        "out-of-bounds negative index at mutation must fail; got {err:?}");
}

#[test]
fn negative_zero_is_rejected_at_parse() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body -> 'tags' -> -0 FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)),
        "`-0` must be rejected; got {err:?}");
}
