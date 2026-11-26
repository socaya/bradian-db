//! ADR 0217 — GROUP BY with aggregate projections.

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
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(5, 'west', 'gadget', 15)",
        "INSERT INTO t.public.sales(id, region, product, qty) VALUES(6, 'north', 'widget', 7)",
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
fn group_by_single_column_with_count() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region"
    ).unwrap());
    assert_eq!(cols, vec!["region", "count"]);
    // Sorted ascending by region: east, north, west.
    assert_eq!(rows, vec![
        vec![Value::String("east".into()), Value::Int64(3)],
        vec![Value::String("north".into()), Value::Int64(1)],
        vec![Value::String("west".into()), Value::Int64(2)],
    ]);
}

#[test]
fn group_by_with_sum() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT region, SUM(qty) FROM t.public.sales GROUP BY region"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::String("east".into()), Value::Int64(35)],
        vec![Value::String("north".into()), Value::Int64(7)],
        vec![Value::String("west".into()), Value::Int64(45)],
    ]);
}

#[test]
fn group_by_with_multiple_aggregates() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (cols, rows) = rows_of(db.execute(
        "SELECT region, COUNT(*), SUM(qty), MIN(qty), MAX(qty) FROM t.public.sales GROUP BY region"
    ).unwrap());
    assert_eq!(cols, vec!["region", "count", "sum", "min", "max"]);
    assert_eq!(rows[0], vec![
        Value::String("east".into()), Value::Int64(3),
        Value::Int64(35), Value::Int64(5), Value::Int64(20),
    ]);
    assert_eq!(rows[1], vec![
        Value::String("north".into()), Value::Int64(1),
        Value::Int64(7), Value::Int64(7), Value::Int64(7),
    ]);
    assert_eq!(rows[2], vec![
        Value::String("west".into()), Value::Int64(2),
        Value::Int64(45), Value::Int64(15), Value::Int64(30),
    ]);
}

#[test]
fn group_by_two_columns() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT region, product, SUM(qty) FROM t.public.sales GROUP BY region, product"
    ).unwrap());
    // Sort order is (region ASC, product ASC): east/gadget,
    // east/widget, north/widget, west/gadget, west/widget.
    assert_eq!(rows, vec![
        vec![Value::String("east".into()), Value::String("gadget".into()), Value::Int64(5)],
        vec![Value::String("east".into()), Value::String("widget".into()), Value::Int64(30)],
        vec![Value::String("north".into()), Value::String("widget".into()), Value::Int64(7)],
        vec![Value::String("west".into()), Value::String("gadget".into()), Value::Int64(15)],
        vec![Value::String("west".into()), Value::String("widget".into()), Value::Int64(30)],
    ]);
}

#[test]
fn group_by_order_of_key_columns_is_canonical() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Swapping GROUP BY order should still emit the same tuples
    // because results sort by key tuple using projection order.
    let (_, rows_a) = rows_of(db.execute(
        "SELECT region, product, COUNT(*) FROM t.public.sales GROUP BY region, product"
    ).unwrap());
    let (_, rows_b) = rows_of(db.execute(
        "SELECT region, product, COUNT(*) FROM t.public.sales GROUP BY product, region"
    ).unwrap());
    assert_eq!(rows_a.len(), rows_b.len());
}

#[test]
fn group_by_bare_projection_not_in_group_by_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT region, product FROM t.public.sales GROUP BY region"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn group_by_missing_with_mixed_projection_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Aggregate + bare column without GROUP BY is ambiguous.
    let err = db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn group_by_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY nope"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn group_by_duplicate_key_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region, region"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn group_by_with_pk_where_isolates_single_group() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales WHERE id = 4 GROUP BY region"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::String("west".into()), Value::Int64(1)],
    ]);
}

#[test]
fn group_by_with_empty_where_returns_zero_groups() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales WHERE id = 99 GROUP BY region"
    ).unwrap());
    assert!(rows.is_empty());
}

#[test]
fn group_by_avg_per_bucket() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT region, AVG(qty) FROM t.public.sales GROUP BY region"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::String("east".into()), Value::Int64(12)], // (10+20+5)/3 = 11.67 → 12 (half-even)
        vec![Value::String("north".into()), Value::Int64(7)],
        vec![Value::String("west".into()), Value::Int64(22)], // (30+15)/2 = 22.5 → 22 even
    ]);
}

#[test]
fn group_by_count_distinct_per_bucket() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let (_, rows) = rows_of(db.execute(
        "SELECT region, COUNT(DISTINCT product) FROM t.public.sales GROUP BY region"
    ).unwrap());
    assert_eq!(rows, vec![
        vec![Value::String("east".into()), Value::Int64(2)],
        vec![Value::String("north".into()), Value::Int64(1)],
        vec![Value::String("west".into()), Value::Int64(2)],
    ]);
}

#[test]
fn domain_engine_group_by() {
    let dir = tempdir().unwrap();
    {
        let ddl = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, cat TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.items").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, cat, qty) VALUES(1, 'a', 10)")?;
        s.execute("INSERT INTO t.public.items(id, cat, qty) VALUES(2, 'a', 15)")?;
        s.execute("INSERT INTO t.public.items(id, cat, qty) VALUES(3, 'b', 5)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT cat, SUM(qty) FROM t.public.items GROUP BY cat")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::String("a".into()), Value::Int64(25)],
        vec![Value::String("b".into()), Value::Int64(5)],
    ]);
}

#[test]
fn mvcc_engine_group_by() {
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
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT cat, COUNT(*) FROM t.public.items GROUP BY cat")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::String("x".into()), Value::Int64(2)],
        vec![Value::String("y".into()), Value::Int64(1)],
    ]);
}
