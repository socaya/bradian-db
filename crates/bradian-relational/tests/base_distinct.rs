//! ADR 0219 — SELECT DISTINCT.

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
        "CREATE TABLE t.public.sales (id BIGINT NOT NULL, region TEXT NOT NULL, product TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(1, 'east', 'widget', 10)",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(2, 'east', 'widget', 20)",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(3, 'east', 'gadget', 5)",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(4, 'west', 'widget', 30)",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(5, 'west', 'widget', 15)",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(6, 'north', 'gadget', 7)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn rows_of(rs: SqlResultSet) -> Vec<Vec<Value>> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows,
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn distinct_single_column() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT DISTINCT region FROM t.public.sales"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into())],
        vec![Value::String("north".into())],
        vec![Value::String("west".into())],
    ]);
}

#[test]
fn distinct_two_columns() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Distinct (region, product) pairs: 4 unique.
    let got = rows_of(db.execute(
        "SELECT DISTINCT region, product FROM t.public.sales"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into()), Value::String("gadget".into())],
        vec![Value::String("east".into()), Value::String("widget".into())],
        vec![Value::String("north".into()), Value::String("gadget".into())],
        vec![Value::String("west".into()), Value::String("widget".into())],
    ]);
}

#[test]
fn distinct_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT DISTINCT qty FROM t.public.sales"
    ).unwrap());
    // Values: 10, 20, 5, 30, 15, 7 → sorted: 5, 7, 10, 15, 20, 30.
    assert_eq!(got, vec![
        vec![Value::Int64(5)],
        vec![Value::Int64(7)],
        vec![Value::Int64(10)],
        vec![Value::Int64(15)],
        vec![Value::Int64(20)],
        vec![Value::Int64(30)],
    ]);
}

#[test]
fn distinct_already_unique_returns_all() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT DISTINCT id FROM t.public.sales"
    ).unwrap());
    assert_eq!(got.len(), 6);
}

#[test]
fn distinct_with_pk_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT DISTINCT region FROM t.public.sales WHERE id = 1"
    ).unwrap());
    assert_eq!(got, vec![vec![Value::String("east".into())]]);
}

#[test]
fn distinct_empty_result() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT DISTINCT region FROM t.public.sales WHERE id = 99"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn distinct_rejected_with_aggregate() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT DISTINCT COUNT(*) FROM t.public.sales"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn distinct_rejected_with_group_by() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT DISTINCT region FROM t.public.sales GROUP BY region"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn distinct_with_nullable_column() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, 'x')",
        "INSERT INTO t.public.items(id, note) VALUES(2, NULL)",
        "INSERT INTO t.public.items(id, note) VALUES(3, 'x')",
        "INSERT INTO t.public.items(id, note) VALUES(4, NULL)",
        "INSERT INTO t.public.items(id, note) VALUES(5, 'y')",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT DISTINCT note FROM t.public.items"
    ).unwrap());
    // Distinct: 'x', 'y', NULL. NULL sorts last; 'x' < 'y'.
    assert_eq!(got, vec![
        vec![Value::String("x".into())],
        vec![Value::String("y".into())],
        vec![Value::Null],
    ]);
}

#[test]
fn distinct_wildcard() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        // Composite PK so duplicate (a,b) is impossible; but
        // adding a row with distinct (id, a, b) pairs still
        // lets DISTINCT run.
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, a TEXT NOT NULL, b TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, a, b) VALUES(1, 'x', 'y')",
        "INSERT INTO t.public.items(id, a, b) VALUES(2, 'x', 'y')",
    ] { db.execute(sql).unwrap(); }
    // SELECT DISTINCT * dedups over the whole row — rows 1 and
    // 2 have the same (a, b) but different id, so both kept.
    let got = rows_of(db.execute("SELECT DISTINCT * FROM t.public.items").unwrap());
    assert_eq!(got.len(), 2);
}

#[test]
fn domain_engine_distinct() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, cat TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(1, 'a')")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(2, 'b')")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(3, 'a')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT DISTINCT cat FROM t.public.items")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::String("a".into())],
        vec![Value::String("b".into())],
    ]);
}

#[test]
fn mvcc_engine_distinct() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, cat TEXT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(1, 'x')")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(2, 'x')")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(3, 'y')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT DISTINCT cat FROM t.public.items")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::String("x".into())],
        vec![Value::String("y".into())],
    ]);
}
