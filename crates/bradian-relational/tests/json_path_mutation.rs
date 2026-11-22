//! ADR 0192 — JSON path mutation on UPDATE SET.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice","level":3}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

fn read_body(db: &SqlEngine, id: i64) -> serde_json::Value {
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
fn set_arrow_key_replaces_scalar_subvalue() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'name' = 'bob' WHERE id=1").unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body["name"], "bob");
    assert_eq!(body["level"], 3);
}

#[test]
fn set_arrow_arrow_key_treats_literal_as_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body ->> 'level' = 'nine' WHERE id=1").unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body["level"], "nine");
}

#[test]
fn set_arrow_with_numeric_literal_writes_json_number() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'level' = 42 WHERE id=1").unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body["level"], 42);
}

#[test]
fn set_arrow_missing_key_creates_it() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body -> 'new' = 'added' WHERE id=1").unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body["new"], "added");
    assert_eq!(body["name"], "alice");
}

#[test]
fn set_arrow_chained_key_auto_creates_intermediate_object() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // body has no `meta` key; mid-chain missing intermediate
    // auto-creates an empty object.
    db.execute("UPDATE t.public.docs SET body -> 'meta' -> 'k' = 'v' WHERE id=1").unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body["meta"]["k"], "v");
}

#[test]
fn set_arrow_array_index_in_bounds_replaces_element() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["red","green","blue"]}')"#,
    ] { db.execute(sql).unwrap(); }
    db.execute("UPDATE t.public.docs SET body -> 'tags' -> 1 = 'yellow' WHERE id=1").unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body["tags"], serde_json::json!(["red", "yellow", "blue"]));
}

#[test]
fn set_arrow_array_index_out_of_bounds_is_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"tags":["x"]}')"#,
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "UPDATE t.public.docs SET body -> 'tags' -> 5 = 'y' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn mix_bare_column_and_json_path_assignments() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, label TEXT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, label, body) VALUES(1, 'a', '{"k":1}')"#,
    ] { db.execute(sql).unwrap(); }
    db.execute(
        "UPDATE t.public.docs SET label='b', body -> 'k' = 2 WHERE id=1"
    ).unwrap();
    match db.execute("SELECT label FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "b"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
    assert_eq!(read_body(&db, 1)["k"], 2);
}

#[test]
fn reset_column_then_mutate_path_runs_in_order() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Reset body to `{}` then set `body -> 'k' = 1`.
    db.execute(
        r#"UPDATE t.public.docs SET body='{}', body -> 'k' = 1 WHERE id=1"#
    ).unwrap();
    let body = read_body(&db, 1);
    assert_eq!(body, serde_json::json!({"k": 1}));
}

#[test]
fn applying_json_path_to_non_json_column_fails_cleanly() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.a(id, label) VALUES(1, 'plain')",
    ] { db.execute(sql).unwrap(); }
    let err = db.execute(
        "UPDATE t.public.a SET label -> 'k' = 'v' WHERE id=1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

// ---- cross-engine confirmations ----

#[test]
fn domain_engine_json_path_mutation() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":1}')"#)?;
        s.execute(r#"UPDATE t.public.docs SET body -> 'k' = 99 WHERE id=1"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["k"], 99);
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
fn mvcc_engine_json_path_mutation() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"nested":{"x":1}}')"#)?;
        s.execute(r#"UPDATE t.public.docs SET body -> 'nested' -> 'x' = 7 WHERE id=1"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(j) => {
                    let parsed: serde_json::Value = serde_json::from_str(j).unwrap();
                    assert_eq!(parsed["nested"]["x"], 7);
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
