//! ADR 0218 — HAVING clause on grouped SELECT.

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
        "CREATE TABLE t.public.sales (id BIGINT NOT NULL, region TEXT NOT NULL, qty BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.sales(id, region, qty) VALUES(1, 'east', 10)",
        "INSERT INTO t.public.sales(id, region, qty) VALUES(2, 'east', 20)",
        "INSERT INTO t.public.sales(id, region, qty) VALUES(3, 'east', 15)",
        "INSERT INTO t.public.sales(id, region, qty) VALUES(4, 'west', 30)",
        "INSERT INTO t.public.sales(id, region, qty) VALUES(5, 'west', 40)",
        "INSERT INTO t.public.sales(id, region, qty) VALUES(6, 'north', 5)",
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
fn having_on_count() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Group counts: east=3, west=2, north=1 — keep > 1 → east, west.
    let got = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region HAVING COUNT(*) > 1"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into()), Value::Int64(3)],
        vec![Value::String("west".into()), Value::Int64(2)],
    ]);
}

#[test]
fn having_on_sum() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Sums: east=45, west=70, north=5 — keep >= 50 → west.
    let got = rows_of(db.execute(
        "SELECT region, SUM(qty) FROM t.public.sales GROUP BY region HAVING SUM(qty) >= 50"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("west".into()), Value::Int64(70)],
    ]);
}

#[test]
fn having_on_group_key() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region HAVING region <> 'north'"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into()), Value::Int64(3)],
        vec![Value::String("west".into()), Value::Int64(2)],
    ]);
}

#[test]
fn having_on_max() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Maxes: east=20, west=40, north=5 — keep > 20 → west.
    let got = rows_of(db.execute(
        "SELECT region, MAX(qty) FROM t.public.sales GROUP BY region HAVING MAX(qty) > 20"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("west".into()), Value::Int64(40)],
    ]);
}

#[test]
fn having_on_min() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Mins: east=10, west=30, north=5 — keep <= 10 → east, north.
    let got = rows_of(db.execute(
        "SELECT region, MIN(qty) FROM t.public.sales GROUP BY region HAVING MIN(qty) <= 10"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into()), Value::Int64(10)],
        vec![Value::String("north".into()), Value::Int64(5)],
    ]);
}

#[test]
fn having_equality_text() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region HAVING region = 'east'"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into()), Value::Int64(3)],
    ]);
}

#[test]
fn having_rejects_non_group_key_column() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region HAVING id > 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn having_rejects_without_group_by() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // HAVING without GROUP BY is parse-rejected via the
    // underlying grammar — HAVING is only consumed after
    // parse_group_by, so the clause is left as trailing tokens.
    let err = db.execute(
        "SELECT region FROM t.public.sales HAVING COUNT(*) > 0"
    ).unwrap_err();
    // Base engine's column projection without GROUP BY fails
    // first on the aggregate-mixing rule since no GROUP BY is
    // present and HAVING is not consumed; trailing HAVING
    // then surfaces as "trailing tokens".
    assert!(matches!(err, SqlError::Unsupported(_) | SqlError::Syntax(_)));
}

#[test]
fn having_count_zero_filters_everything() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales GROUP BY region HAVING COUNT(*) > 100"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn having_with_where_filters_rows_then_buckets() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // WHERE id = 4 keeps only row id=4 (west, 30). HAVING
    // COUNT(*) >= 1 keeps the west bucket.
    let got = rows_of(db.execute(
        "SELECT region, COUNT(*) FROM t.public.sales WHERE id = 4 GROUP BY region HAVING COUNT(*) >= 1"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("west".into()), Value::Int64(1)],
    ]);
}

#[test]
fn having_distinct_count() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.sales (id BIGINT NOT NULL, region TEXT NOT NULL, product TEXT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.sales(id, region, product) VALUES(1, 'east', 'a')",
        "INSERT INTO t.public.sales(id, region, product) VALUES(2, 'east', 'a')",
        "INSERT INTO t.public.sales(id, region, product) VALUES(3, 'east', 'b')",
        "INSERT INTO t.public.sales(id, region, product) VALUES(4, 'west', 'c')",
    ] { db.execute(sql).unwrap(); }
    let got = rows_of(db.execute(
        "SELECT region, COUNT(DISTINCT product) FROM t.public.sales GROUP BY region HAVING COUNT(DISTINCT product) >= 2"
    ).unwrap());
    assert_eq!(got, vec![
        vec![Value::String("east".into()), Value::Int64(2)],
    ]);
}

#[test]
fn domain_engine_having() {
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
        s.execute("INSERT INTO t.public.items(id, cat, qty) VALUES(2, 'a', 10)")?;
        s.execute("INSERT INTO t.public.items(id, cat, qty) VALUES(3, 'b', 50)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT cat, SUM(qty) FROM t.public.items GROUP BY cat HAVING SUM(qty) >= 30")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::String("b".into()), Value::Int64(50)],
    ]);
}

#[test]
fn mvcc_engine_having() {
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
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(4, 'y')")?;
        s.execute("INSERT INTO t.public.items(id, cat) VALUES(5, 'y')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got: std::sync::Arc<std::sync::Mutex<Vec<Vec<Value>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute("SELECT cat, COUNT(*) FROM t.public.items GROUP BY cat HAVING COUNT(*) > 2")?
        {
            *captured.lock().unwrap() = rows;
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![
        vec![Value::String("y".into()), Value::Int64(3)],
    ]);
}
