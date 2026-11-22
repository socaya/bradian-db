//! ADR 0201 — JSON array INSERT (splice) on UPDATE SET.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
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

fn tags(db: &SqlEngine, id: i64) -> serde_json::Value {
    match db.execute(&format!("SELECT body -> 'tags' FROM t.public.docs WHERE id={id}")).unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::Json(s) => serde_json::from_str(s).unwrap(),
            other => panic!("expected Json; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn insert_at_beginning_prepends() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 0 INSERT 'yellow' WHERE id=1").unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["yellow", "red", "green", "blue"]));
}

#[test]
fn insert_middle_shifts_right() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 1 INSERT 'yellow' WHERE id=1").unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red", "yellow", "green", "blue"]));
}

#[test]
fn insert_at_length_is_append() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 3 INSERT 'yellow' WHERE id=1").unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red", "green", "blue", "yellow"]));
}

#[test]
fn insert_into_empty_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 0 INSERT 'only' WHERE id=2").unwrap();
    assert_eq!(tags(&db, 2), serde_json::json!(["only"]));
}

#[test]
fn insert_arrow_arrow_text_writes_string_tag() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' ->> 1 INSERT 42 WHERE id=1").unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red", "42", "green", "blue"]));
}

#[test]
fn insert_via_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body #> '{tags,1}' INSERT 'yellow' WHERE id=1").unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red", "yellow", "green", "blue"]));
}

#[test]
fn insert_past_length_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 99 INSERT 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn insert_negative_index_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // IndexFromEnd on an INSERT terminal — rejected (ambiguous
    // which way "shift" should go).
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> -1 INSERT 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn insert_slice_segment_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 0:2 INSERT 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn insert_object_key_terminal_is_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"nested":{"k":1}}')"#,
    ] { db.execute(sql).unwrap(); }
    // INSERT at a key does not make sense (keys don't have a
    // position to shift).
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'nested' -> 'k' INSERT 'nope' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn insert_combines_with_remove_in_one_set() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Remove index 0 then insert 'start' at new index 0.
    db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 0 REMOVE, body -> 'tags' -> 0 INSERT 'start' WHERE id=1"
    ).unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["start", "green", "blue"]));
}

#[test]
fn domain_engine_insert() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.docs").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["a","b"]}')"#)?;
        s.execute("UPDATE t.public.docs SET body -> 'tags' -> 1 INSERT 'mid' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["tags"], serde_json::json!(["a", "mid", "b"]));
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_engine_insert() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["a","b"]}')"#)?;
        s.execute("UPDATE t.public.docs SET body -> 'tags' -> 0 INSERT 'first' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["tags"], serde_json::json!(["first", "a", "b"]));
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
