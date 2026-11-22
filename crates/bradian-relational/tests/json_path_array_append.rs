//! ADR 0198 — JSON array append via `SET col -> 'arr' -> N`
//! where `N == arr.len()`.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["red","green"]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tags":[]}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn body(db: &SqlEngine, id: i64) -> serde_json::Value {
    match db.execute(&format!("SELECT body FROM t.public.docs WHERE id={id}")).unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => serde_json::from_str(s).unwrap(),
            Value::String(s) => serde_json::from_str(s).unwrap(),
            other => panic!("expected json cell; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn append_via_index_at_length() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 2 = 'blue' WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"], serde_json::json!(["red","green","blue"]));
}

#[test]
fn append_to_empty_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 0 = 'first' WHERE id=2").unwrap();
    assert_eq!(body(&db, 2)["tags"], serde_json::json!(["first"]));
}

#[test]
fn multiple_appends_build_up_the_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Length 2 → 3 → 4.
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 2 = 'blue' WHERE id=1").unwrap();
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 3 = 'yellow' WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"],
        serde_json::json!(["red","green","blue","yellow"]));
}

#[test]
fn append_with_arrow_arrow_text_writes_string_tag() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ->> at the terminal segment writes a JSON string even if
    // the literal looks numeric.
    db.execute("UPDATE t.public.docs SET body -> 'tags' ->> 2 = 42 WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"], serde_json::json!(["red","green","42"]));
}

#[test]
fn append_via_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body #> '{tags,2}' = 'blue' WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"], serde_json::json!(["red","green","blue"]));
}

#[test]
fn index_one_past_length_still_rejects() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Length 2 → index 3 would leave a hole at index 2.
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 3 = 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)),
        "index > len must still reject; got {err:?}");
}

#[test]
fn negative_index_cannot_append() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // IndexFromEnd never addresses a hypothetical slot.
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> -3 = 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)),
        "negative index must stay out-of-bounds at mutation; got {err:?}");
}

#[test]
fn midchain_index_at_len_is_still_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"outer":[[0,1],[2,3]]}')"#,
    ] { db.execute(sql).unwrap(); }
    // Writing outer[2]['sub'] = X requires creating a new outer
    // slot AND descending into it. Only the terminal segment
    // can be an append; mid-chain still rejects.
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'outer' -> 2 -> 0 = 9 WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)),
        "mid-chain index == len must reject; got {err:?}");
}

#[test]
fn append_round_trips_via_negative_last_read() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 2 = 'blue' WHERE id=1").unwrap();
    // Read the last element back via -1.
    match db.execute("SELECT body -> 'tags' ->> -1 FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "blue"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}
