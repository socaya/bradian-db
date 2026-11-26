//! ADR 0214 — MIN, MAX, SUM aggregates on the base engines.

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
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, qty BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, qty, note) VALUES(1, 'alpha', 10, 'first')",
        "INSERT INTO t.public.items(id, name, qty, note) VALUES(2, 'bravo', 25, NULL)",
        "INSERT INTO t.public.items(id, name, qty, note) VALUES(3, 'charlie', 5, 'third')",
        "INSERT INTO t.public.items(id, name, qty, note) VALUES(4, 'delta', 40, NULL)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn single(rs: SqlResultSet, col: &str) -> Value {
    match rs {
        SqlResultSet::Rows { columns, rows, .. } => {
            assert_eq!(columns, vec![col.to_string()]);
            assert_eq!(rows.len(), 1);
            rows[0][0].clone()
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn min_on_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT MIN(qty) FROM t.public.items").unwrap(), "min"),
        Value::Int64(5),
    );
}

#[test]
fn max_on_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT MAX(qty) FROM t.public.items").unwrap(), "max"),
        Value::Int64(40),
    );
}

#[test]
fn sum_on_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT SUM(qty) FROM t.public.items").unwrap(), "sum"),
        Value::Int64(80),
    );
}

#[test]
fn min_on_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT MIN(name) FROM t.public.items").unwrap(), "min"),
        Value::String("alpha".into()),
    );
}

#[test]
fn max_on_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT MAX(name) FROM t.public.items").unwrap(), "max"),
        Value::String("delta".into()),
    );
}

#[test]
fn min_skips_nulls() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // note has two NULLs plus 'first' and 'third'.
    assert_eq!(
        single(db.execute("SELECT MIN(note) FROM t.public.items").unwrap(), "min"),
        Value::String("first".into()),
    );
}

#[test]
fn min_on_all_null_is_null() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, n BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, n) VALUES(1, NULL)",
        "INSERT INTO t.public.items(id, n) VALUES(2, NULL)",
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        single(db.execute("SELECT MIN(n) FROM t.public.items").unwrap(), "min"),
        Value::Null,
    );
}

#[test]
fn sum_on_empty_is_null() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, n BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        single(db.execute("SELECT SUM(n) FROM t.public.items").unwrap(), "sum"),
        Value::Null,
    );
}

#[test]
fn sum_rejects_non_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT SUM(name) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn aggregate_with_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // qty filter > 10 keeps 25, 40 → sum 65.
    // Base engine WHERE only supports PK equality, so route via
    // PK selection instead: id=2 has qty=25.
    assert_eq!(
        single(db.execute("SELECT MAX(qty) FROM t.public.items WHERE id = 2").unwrap(), "max"),
        Value::Int64(25),
    );
}

#[test]
fn aggregate_with_pk_miss_returns_null_or_zero() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT SUM(qty) FROM t.public.items WHERE id = 99").unwrap(), "sum"),
        Value::Null,
    );
}

#[test]
fn min_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT MIN(nope) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn domain_engine_sum() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 100)")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(2, 200)")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(3, 300)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Value::Null));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT SUM(qty) FROM t.public.items")?
        {
            *captured.lock().unwrap() = rows[0][0].clone();
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), Value::Int64(600));
}

#[test]
fn mvcc_engine_min_max() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.items").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 7)")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(2, 22)")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(3, 3)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got_min = std::sync::Arc::new(std::sync::Mutex::new(Value::Null));
    let got_max = std::sync::Arc::new(std::sync::Mutex::new(Value::Null));
    let cmin = got_min.clone();
    let cmax = got_max.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT MIN(qty) FROM t.public.items")? {
            *cmin.lock().unwrap() = rows[0][0].clone();
        }
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT MAX(qty) FROM t.public.items")? {
            *cmax.lock().unwrap() = rows[0][0].clone();
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got_min.lock().unwrap(), Value::Int64(3));
    assert_eq!(*got_max.lock().unwrap(), Value::Int64(22));
}
