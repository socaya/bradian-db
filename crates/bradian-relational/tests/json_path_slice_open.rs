//! ADR 0202 — open-ended JSON slices (`-> :n`, `-> n:`, `-> :`).

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["red","green","blue","yellow","black"]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tags":[]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"notes":"string"}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn jrows(db: &SqlEngine, sql: &str) -> serde_json::Value {
    match db.execute(sql).unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => serde_json::from_str(s).unwrap(),
            other => panic!("expected Json; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn open_end_runs_to_length() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> 1: FROM t.public.docs WHERE id=1"),
        serde_json::json!(["green","blue","yellow","black"]),
    );
}

#[test]
fn open_start_runs_from_zero() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> :3 FROM t.public.docs WHERE id=1"),
        serde_json::json!(["red","green","blue"]),
    );
}

#[test]
fn both_open_is_whole_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> : FROM t.public.docs WHERE id=1"),
        serde_json::json!(["red","green","blue","yellow","black"]),
    );
}

#[test]
fn open_end_on_empty_array_returns_empty() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> 0: FROM t.public.docs WHERE id=2"),
        serde_json::json!([]),
    );
}

#[test]
fn open_end_past_length_clamps() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // start past end → empty
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> 99: FROM t.public.docs WHERE id=1"),
        serde_json::json!([]),
    );
}

#[test]
fn open_start_past_zero_is_prefix() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // open start equivalent to 0:2.
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> :2 FROM t.public.docs WHERE id=1"),
        serde_json::json!(["red","green"]),
    );
}

#[test]
fn chained_open_slice_then_index() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Slice first, then pick an element from the materialized
    // array.
    assert_eq!(
        jrows(&db, "SELECT body -> 'tags' -> 1: -> 0 FROM t.public.docs WHERE id=1"),
        serde_json::json!("green"),
    );
}

#[test]
fn open_slice_on_non_array_is_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body -> 'notes' -> 0: FROM t.public.docs WHERE id=3").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_open_end() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body #> '{tags,2:}' FROM t.public.docs WHERE id=1"),
        serde_json::json!(["blue","yellow","black"]),
    );
}

#[test]
fn container_open_start() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body #> '{tags,:2}' FROM t.public.docs WHERE id=1"),
        serde_json::json!(["red","green"]),
    );
}

#[test]
fn container_both_open() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        jrows(&db, "SELECT body #> '{tags,:}' FROM t.public.docs WHERE id=1"),
        serde_json::json!(["red","green","blue","yellow","black"]),
    );
}

#[test]
fn open_slice_on_update_lhs_still_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ADR 0197 — slice targets on SET LHS remain out of scope.
    let err = db.execute("UPDATE t.public.docs SET body -> 'tags' -> 0: = '[]' WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn open_container_slice_bad_text_segment_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body #> '{tags,a:2}' FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}
