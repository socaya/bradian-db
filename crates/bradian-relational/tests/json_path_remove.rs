//! ADR 0200 — JSON element REMOVE on UPDATE SET.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":1,"name":"alice","tags":["red","green","blue"]}')"#,
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
fn remove_object_key() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'name' REMOVE WHERE id=1").unwrap();
    let out = body(&db, 1);
    assert!(out.get("name").is_none());
    assert_eq!(out["k"], 1);
    assert_eq!(out["tags"].as_array().unwrap().len(), 3);
}

#[test]
fn remove_missing_object_key_is_idempotent() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'nope' REMOVE WHERE id=1").unwrap();
    // Original keys preserved.
    let out = body(&db, 1);
    assert!(out.get("name").is_some());
}

#[test]
fn remove_array_element_shifts_left() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 1 REMOVE WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"], serde_json::json!(["red", "blue"]));
}

#[test]
fn remove_last_element_by_negative_index() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> -1 REMOVE WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"], serde_json::json!(["red", "green"]));
}

#[test]
fn remove_via_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body #> '{tags,0}' REMOVE WHERE id=1").unwrap();
    assert_eq!(body(&db, 1)["tags"], serde_json::json!(["green", "blue"]));
}

#[test]
fn remove_array_index_out_of_bounds_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 99 REMOVE WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn remove_slice_segment_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 0:2 REMOVE WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn remove_with_json_path_where() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tier":"a","k":1}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tier":"b","k":2}')"#,
    ] { db.execute(sql).unwrap(); }
    db.execute(
        "UPDATE t.public.docs SET body -> 'k' REMOVE WHERE body ->> 'tier' = 'a'"
    ).unwrap();
    let got1 = body(&db, 1);
    assert!(got1.get("k").is_none());
    let got2 = body(&db, 2);
    assert_eq!(got2["k"], 2);
}

#[test]
fn remove_combines_with_other_assignments_in_one_set() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'new' = 'added', body -> 'name' REMOVE WHERE id=1"
    ).unwrap();
    let out = body(&db, 1);
    assert_eq!(out["new"], "added");
    assert!(out.get("name").is_none());
}

#[test]
fn remove_on_non_json_column_fails_cleanly() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id, label) VALUES(1, 'plain')",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "UPDATE t.public.a SET label -> 'k' REMOVE WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

// ---- Cross-engine sanity ----

#[test]
fn domain_engine_remove() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":1,"j":2}')"#)?;
        s.execute("UPDATE t.public.docs SET body -> 'k' REMOVE WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert!(parsed.get("k").is_none());
                    assert_eq!(parsed["j"], 2);
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
fn mvcc_engine_remove() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["a","b","c"]}')"#)?;
        s.execute("UPDATE t.public.docs SET body -> 'tags' -> 1 REMOVE WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["tags"], serde_json::json!(["a", "c"]));
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
