//! ADR 0197 — JSON array slice indices.

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
    ] { db.execute(sql).unwrap(); }
    db
}

fn json_rows(db: &SqlEngine, sql: &str) -> serde_json::Value {
    match db.execute(sql).unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => serde_json::from_str(s).unwrap(),
            other => panic!("expected Json; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn arrow_slice_returns_sub_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body -> 'tags' -> 1:4 FROM t.public.docs WHERE id=1"),
        serde_json::json!(["green","blue","yellow"]),
    );
}

#[test]
fn arrow_slice_from_zero_matches_prefix() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body -> 'tags' -> 0:2 FROM t.public.docs WHERE id=1"),
        serde_json::json!(["red","green"]),
    );
}

#[test]
fn arrow_slice_past_end_clamps() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 3..99 on a length-5 array clamps to 3..5.
    assert_eq!(
        json_rows(&db, "SELECT body -> 'tags' -> 3:99 FROM t.public.docs WHERE id=1"),
        serde_json::json!(["yellow","black"]),
    );
}

#[test]
fn arrow_slice_equal_bounds_returns_empty_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body -> 'tags' -> 2:2 FROM t.public.docs WHERE id=1"),
        serde_json::json!([]),
    );
}

#[test]
fn arrow_slice_on_empty_array_returns_empty_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body -> 'tags' -> 0:3 FROM t.public.docs WHERE id=2"),
        serde_json::json!([]),
    );
}

#[test]
fn arrow_slice_on_non_array_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // body is an object, not array.
    match db.execute("SELECT body -> 0:2 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn chained_slice_then_index_extracts_single_element() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // tags[1:4] = [green, blue, yellow]; -> 0 is "green".
    match db.execute("SELECT body -> 'tags' -> 1:4 ->> 0 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "green"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_slice_segment() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body #> '{tags,1:3}' FROM t.public.docs WHERE id=1"),
        serde_json::json!(["green","blue"]),
    );
}

#[test]
fn slice_start_greater_than_end_rejected_at_parse() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body -> 'tags' -> 4:2 FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn slice_open_end_runs_to_array_length() {
    // ADR 0202 — `-> 2:` means "from index 2 to the end".
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body -> 'tags' -> 2: FROM t.public.docs WHERE id=1"),
        serde_json::json!(["blue","yellow","black"]),
    );
}

#[test]
fn slice_container_open_end_runs_to_array_length() {
    // ADR 0202 — container form with an omitted end.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        json_rows(&db, "SELECT body #> '{tags,1:}' FROM t.public.docs WHERE id=1"),
        serde_json::json!(["green","blue","yellow","black"]),
    );
}

#[test]
fn slice_on_update_lhs_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Replacing an array slice on SET LHS is intentionally
    // out of scope.
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 0:2 = '[\"x\",\"y\"]' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)),
        "slice mutation must be rejected; got {err:?}");
}
