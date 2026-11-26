//! ADR 0215 — AVG scalar aggregate.

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
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, qty, note) VALUES(1, 10, NULL)",
        "INSERT INTO t.public.items(id, qty, note) VALUES(2, 20, 'a')",
        "INSERT INTO t.public.items(id, qty, note) VALUES(3, 30, NULL)",
        "INSERT INTO t.public.items(id, qty, note) VALUES(4, 40, 'b')",
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
fn avg_integer_truncates_evenly() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // (10+20+30+40) / 4 = 25.
    assert_eq!(
        single(db.execute("SELECT AVG(qty) FROM t.public.items").unwrap(), "avg"),
        Value::Int64(25),
    );
}

#[test]
fn avg_integer_rounds_half_to_even() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, qty) VALUES(1, 1)",
        "INSERT INTO t.public.items(id, qty) VALUES(2, 2)",
    ] { db.execute(sql).unwrap(); }
    // Exact half 1.5 → banker's rounds to 2 (nearest even).
    assert_eq!(
        single(db.execute("SELECT AVG(qty) FROM t.public.items").unwrap(), "avg"),
        Value::Int64(2),
    );
}

#[test]
fn avg_integer_rounds_half_to_even_down_to_zero() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, qty) VALUES(1, 0)",
        "INSERT INTO t.public.items(id, qty) VALUES(2, 1)",
    ] { db.execute(sql).unwrap(); }
    // 0.5 → nearest even is 0.
    assert_eq!(
        single(db.execute("SELECT AVG(qty) FROM t.public.items").unwrap(), "avg"),
        Value::Int64(0),
    );
}

#[test]
fn avg_skips_nulls() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, n BIGINT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, n) VALUES(1, 10)",
        "INSERT INTO t.public.items(id, n) VALUES(2, NULL)",
        "INSERT INTO t.public.items(id, n) VALUES(3, 30)",
    ] { db.execute(sql).unwrap(); }
    // (10 + 30) / 2 = 20; NULL excluded.
    assert_eq!(
        single(db.execute("SELECT AVG(n) FROM t.public.items").unwrap(), "avg"),
        Value::Int64(20),
    );
}

#[test]
fn avg_on_empty_is_null() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    assert_eq!(
        single(db.execute("SELECT AVG(qty) FROM t.public.items").unwrap(), "avg"),
        Value::Null,
    );
}

#[test]
fn avg_all_null_is_null() {
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
        single(db.execute("SELECT AVG(n) FROM t.public.items").unwrap(), "avg"),
        Value::Null,
    );
}

#[test]
fn avg_on_decimal_preserves_scale() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, price DECIMAL(10,2) NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, price) VALUES(1, 100.00)",
        "INSERT INTO t.public.items(id, price) VALUES(2, 150.50)",
    ] { db.execute(sql).unwrap(); }
    // (10000 + 15050) / 2 = 12525 → Decimal 125.25 → i128 12525.
    assert_eq!(
        single(db.execute("SELECT AVG(price) FROM t.public.items").unwrap(), "avg"),
        Value::Decimal(12525),
    );
}

#[test]
fn avg_rejects_non_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT AVG(note) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Type(_)));
}

#[test]
fn avg_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute("SELECT AVG(nope) FROM t.public.items").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn avg_with_pk_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    assert_eq!(
        single(db.execute("SELECT AVG(qty) FROM t.public.items WHERE id = 2").unwrap(), "avg"),
        Value::Int64(20),
    );
}

#[test]
fn avg_negative_integer_round_half_even() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, qty) VALUES(1, -1)",
        "INSERT INTO t.public.items(id, qty) VALUES(2, -2)",
    ] { db.execute(sql).unwrap(); }
    // -1.5 → nearest even is -2.
    assert_eq!(
        single(db.execute("SELECT AVG(qty) FROM t.public.items").unwrap(), "avg"),
        Value::Int64(-2),
    );
}

#[test]
fn domain_engine_avg() {
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
            s.execute("SELECT AVG(qty) FROM t.public.items")?
        {
            *captured.lock().unwrap() = rows[0][0].clone();
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), Value::Int64(200));
}

#[test]
fn mvcc_engine_avg() {
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
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(1, 5)")?;
        s.execute("INSERT INTO t.public.items(id, qty) VALUES(2, 15)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Value::Null));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } = s.execute("SELECT AVG(qty) FROM t.public.items")? {
            *captured.lock().unwrap() = rows[0][0].clone();
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), Value::Int64(10));
}
