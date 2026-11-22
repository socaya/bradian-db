//! ADR 0194 — `#>` / `#>>` container-path operators.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":{"b":{"c":"deep"},"n":42}}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"tags":["red","green","blue"]}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn container_hash_returns_json_subvalue() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body #> '{a,b}' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body#>'{a,b}'".to_string()]);
            match &rows[0][0] {
                Value::Json(s) => {
                    let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                    assert_eq!(parsed["c"], "deep");
                }
                other => panic!("expected Json; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_hash_hash_returns_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body #>> '{a,b,c}' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body#>>'{a,b,c}'".to_string()]);
            match &rows[0][0] {
                Value::String(s) => assert_eq!(s, "deep"),
                other => panic!("expected String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_hash_supports_array_index_segments() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body #>> '{tags,1}' FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "green"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_hash_missing_key_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT body #> '{nope,here}' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_hash_is_whitespace_tolerant() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Spaces around commas and braces are fine.
    match db.execute("SELECT body #>> '{ a , b , c }' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "deep"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_hash_rejects_missing_braces() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body #> 'a,b' FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn container_hash_rejects_empty_body() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body #> '{}' FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn container_hash_rejects_empty_segment() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT body #> '{a,,b}' FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn container_hash_in_where_filter() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    match db.execute("SELECT id FROM t.public.docs WHERE body #>> '{a,b,c}' = 'deep'").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(1));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn container_hash_in_update_mutation() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute("UPDATE t.public.docs SET body #> '{a,b,c}' = 'changed' WHERE id=1").unwrap();
    match db.execute("SELECT body #>> '{a,b,c}' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "changed"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

// ---- cross-engine sanity ----

#[test]
fn domain_container_hash() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"x":{"y":"z"}}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body #>> '{x,y}' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::String(ref v) => assert_eq!(v, "z"),
                other => panic!("expected String; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_container_hash() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"x":{"y":"z"}}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body #>> '{x,y}' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::String(ref v) => assert_eq!(v, "z"),
                other => panic!("expected String; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
