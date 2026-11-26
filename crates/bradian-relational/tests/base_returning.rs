//! ADR 0221 — RETURNING on INSERT / UPDATE / DELETE.

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
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'alpha', 10)",
        "INSERT INTO t.public.items(id, name, qty) VALUES(2, 'bravo', 20)",
        "INSERT INTO t.public.items(id, name, qty) VALUES(3, 'charlie', 30)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> (Vec<String>, Vec<Vec<Value>>) {
    match rs {
        SqlResultSet::Rows { columns, rows, .. } => (columns, rows),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn insert_returning_star() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let (cols, rows) = rows_of(db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'x'), (2, 'y') RETURNING *"
    ).unwrap());
    assert_eq!(cols, vec!["id", "name"]);
    assert_eq!(rows, vec![
        vec![Value::Int64(1), Value::String("x".into())],
        vec![Value::Int64(2), Value::String("y".into())],
    ]);
}

#[test]
fn insert_returning_subset() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let (cols, rows) = rows_of(db.execute(
        "INSERT INTO t.public.items(id, name, qty) VALUES(1, 'a', 10) RETURNING id, qty"
    ).unwrap());
    assert_eq!(cols, vec!["id", "qty"]);
    assert_eq!(rows, vec![vec![Value::Int64(1), Value::Int64(10)]]);
}

#[test]
fn update_returning_mutated_row() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "UPDATE t.public.items SET name = 'renamed' WHERE id = 2 RETURNING id, name"
    ).unwrap());
    assert_eq!(cols, vec!["id", "name"]);
    assert_eq!(rows, vec![vec![Value::Int64(2), Value::String("renamed".into())]]);
}

#[test]
fn delete_returning_removed_row() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "DELETE FROM t.public.items WHERE id = 1 RETURNING id, name"
    ).unwrap());
    assert_eq!(cols, vec!["id", "name"]);
    assert_eq!(rows, vec![vec![Value::Int64(1), Value::String("alpha".into())]]);
    // Row is gone.
    let after = db.execute("SELECT COUNT(*) FROM t.public.items").unwrap();
    if let SqlResultSet::Rows { rows, .. } = after {
        assert_eq!(rows[0][0], Value::Int64(2));
    }
}

#[test]
fn delete_returning_star() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "DELETE FROM t.public.items WHERE id = 3 RETURNING *"
    ).unwrap());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 3);
}

#[test]
fn insert_without_returning_still_returns_command() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    match db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap() {
        SqlResultSet::Command { affected } => assert_eq!(affected, 1),
        other => panic!("expected Command; got {other:?}"),
    }
}

#[test]
fn returning_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "DELETE FROM t.public.items WHERE id = 1 RETURNING nope"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn update_returning_empty_when_no_match() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "UPDATE t.public.items SET name = 'x' WHERE id = 99 RETURNING id"
    ).unwrap());
    assert!(rows.is_empty());
}

#[test]
fn insert_returning_multi_row() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let (_, rows) = rows_of(db.execute(
        "INSERT INTO t.public.items(id, name) VALUES(1, 'a'), (2, 'b'), (3, 'c') RETURNING id"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::Int64(1)],
        vec![Value::Int64(2)],
        vec![Value::Int64(3)],
    ]);
}

#[test]
fn domain_engine_insert_returning() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'alpha') RETURNING id, name")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::Int64(1), Value::String("alpha".into())],
    ]);
}

#[test]
fn mvcc_engine_update_returning() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'old')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("UPDATE t.public.items SET name = 'new' WHERE id = 1 RETURNING id, name")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::Int64(1), Value::String("new".into())],
    ]);
}
