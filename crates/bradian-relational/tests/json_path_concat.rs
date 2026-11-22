//! ADR 0203 — JSON array CONCAT on UPDATE SET.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["red","green"]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tags":[]}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"notes":"string"}')"#,
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
fn concat_appends_array_literal() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'tags' CONCAT '[\"blue\",\"black\"]' WHERE id=1"
    ).unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red","green","blue","black"]));
}

#[test]
fn concat_empty_rhs_is_noop() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'tags' CONCAT '[]' WHERE id=1"
    ).unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red","green"]));
}

#[test]
fn concat_onto_empty_array() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'tags' CONCAT '[\"first\"]' WHERE id=2"
    ).unwrap();
    assert_eq!(tags(&db, 2), serde_json::json!(["first"]));
}

#[test]
fn concat_mixed_element_types() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'tags' CONCAT '[1, true, null, {\"k\":2}]' WHERE id=1"
    ).unwrap();
    assert_eq!(
        tags(&db, 1),
        serde_json::json!(["red","green",1,true,null,{"k":2}]),
    );
}

#[test]
fn concat_non_array_rhs_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' CONCAT '{\"k\":1}' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn concat_non_array_target_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'notes' CONCAT '[\"x\"]' WHERE id=3"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn concat_bad_json_rhs_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' CONCAT 'not a json array' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn concat_combines_with_insert() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // INSERT at 0, then CONCAT — CONCAT runs after INSERT.
    db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 0 INSERT 'first', body -> 'tags' CONCAT '[\"last\"]' WHERE id=1"
    ).unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["first","red","green","last"]));
}

#[test]
fn concat_via_container_path() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body #> '{tags}' CONCAT '[\"x\"]' WHERE id=1"
    ).unwrap();
    assert_eq!(tags(&db, 1), serde_json::json!(["red","green","x"]));
}

#[test]
fn concat_missing_intermediate_key_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'absent' -> 'nope' CONCAT '[1]' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn concat_slice_segment_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 0:2 CONCAT '[1]' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn domain_engine_concat() {
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
        s.execute("UPDATE t.public.docs SET body -> 'tags' CONCAT '[\"c\",\"d\"]' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["tags"], serde_json::json!(["a","b","c","d"]));
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
fn mvcc_engine_concat() {
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
        s.execute("UPDATE t.public.docs SET body -> 'tags' CONCAT '[\"z\"]' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["tags"], serde_json::json!(["a","b","z"]));
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
