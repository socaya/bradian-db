//! ADR 0230 — UNION and INSERT SELECT in transactional engines.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn domain_with(dir: &std::path::Path, ddl: &[&str]) -> DomainSqlEngine {
    {
        let ddl_engine = SqlEngine::open(dir).unwrap();
        for sql in ddl { ddl_engine.execute(sql).unwrap(); }
    }
    DomainSqlEngine::open(dir).unwrap()
}

#[test]
fn domain_engine_union_combines_rows() {
    let dir = tempdir().unwrap();
    let dom = domain_with(dir.path(), &[
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ]);
    dom.register_catalog_table("t.public.a").unwrap();
    dom.register_catalog_table("t.public.b").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.a(id) VALUES(1), (2)")?;
        s.execute("INSERT INTO t.public.b(id) VALUES(2), (3)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute(
            "SELECT id FROM t.public.a UNION SELECT id FROM t.public.b"
        )? {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::Int64(1)],
        vec![Value::Int64(2)],
        vec![Value::Int64(3)],
    ]);
}

#[test]
fn domain_engine_insert_select_copies_rows() {
    let dir = tempdir().unwrap();
    let dom = domain_with(dir.path(), &[
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.src (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.dst (id BIGINT NOT NULL, name TEXT NOT NULL, PRIMARY KEY(id))",
    ]);
    dom.register_catalog_table("t.public.src").unwrap();
    dom.register_catalog_table("t.public.dst").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.src(id, name) VALUES(1, 'a'), (2, 'b')")?;
        s.execute("INSERT INTO t.public.dst(id, name) SELECT id, name FROM t.public.src")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<i64>> = std::sync::Arc::new(std::sync::Mutex::new(0));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT COUNT(*) FROM t.public.dst")? {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 2);
}

#[test]
fn mvcc_engine_union() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.a (id BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE t.public.b (id BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.a").unwrap();
        dom.register_catalog_table("t.public.b").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.a(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.b(id) VALUES(2)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute(
            "SELECT id FROM t.public.a UNION ALL SELECT id FROM t.public.b"
        )? {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::Int64(1)],
        vec![Value::Int64(2)],
    ]);
}

#[test]
fn mvcc_engine_insert_select() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.src (id BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE t.public.dst (id BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.src").unwrap();
        dom.register_catalog_table("t.public.dst").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.src(id) VALUES(1), (2), (3)")?;
        s.execute("INSERT INTO t.public.dst(id) SELECT id FROM t.public.src WHERE id = 2")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<i64>> = std::sync::Arc::new(std::sync::Mutex::new(0));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT COUNT(*) FROM t.public.dst")? {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 1);
}

#[test]
fn domain_engine_insert_select_sees_session_inserts() {
    let dir = tempdir().unwrap();
    let dom = domain_with(dir.path(), &[
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.src (id BIGINT NOT NULL, PRIMARY KEY(id))",
        "CREATE TABLE t.public.dst (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ]);
    dom.register_catalog_table("t.public.src").unwrap();
    dom.register_catalog_table("t.public.dst").unwrap();
    // INSERT INTO src and immediately INSERT SELECT in the
    // same session — the copy must see the session's own
    // staged inserts.
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.src(id) VALUES(10), (20)")?;
        s.execute("INSERT INTO t.public.dst(id) SELECT id FROM t.public.src")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<i64>> = std::sync::Arc::new(std::sync::Mutex::new(0));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT COUNT(*) FROM t.public.dst")? {
            if let Value::Int64(n) = rows[0][0] { *captured.lock().unwrap() = n; }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), 2);
}
