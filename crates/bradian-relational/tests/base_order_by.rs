//! ADR 0212 — ORDER BY / LIMIT / OFFSET on the base engines.

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
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, name TEXT NOT NULL, price BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, name, price) VALUES(1, 'bread', 3)",
        "INSERT INTO t.public.items(id, name, price) VALUES(2, 'apple', 1)",
        "INSERT INTO t.public.items(id, name, price) VALUES(3, 'cherry', 2)",
        "INSERT INTO t.public.items(id, name, price) VALUES(4, 'apple', 4)",
        "INSERT INTO t.public.items(id, name, price) VALUES(5, 'date', 5)",
    ] { db.execute(sql).unwrap(); }
    db
}

fn names(rs: SqlResultSet) -> Vec<String> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows.into_iter()
            .map(|r| match &r[0] { Value::String(v) => v.clone(), _ => panic!("expected text") })
            .collect(),
        other => panic!("expected Rows; got {other:?}"),
    }
}

fn ids(rs: SqlResultSet) -> Vec<i64> {
    match rs {
        SqlResultSet::Rows { rows, .. } => rows.into_iter()
            .map(|r| match r[0] { Value::Int64(v) => v, _ => panic!("expected id") })
            .collect(),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn order_by_text_ascending_default() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = names(db.execute(
        "SELECT name FROM t.public.items ORDER BY name"
    ).unwrap());
    assert_eq!(got, vec!["apple", "apple", "bread", "cherry", "date"]);
}

#[test]
fn order_by_text_descending() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = names(db.execute(
        "SELECT name FROM t.public.items ORDER BY name DESC"
    ).unwrap());
    assert_eq!(got, vec!["date", "cherry", "bread", "apple", "apple"]);
}

#[test]
fn order_by_numeric() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items ORDER BY price"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 1, 4, 5]);
}

#[test]
fn order_by_pk_tiebreak_when_sort_key_ties() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Two "apple" rows: id=2 and id=4. Ordering by name puts
    // apples first with PK tie-break by id ASC.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items ORDER BY name"
    ).unwrap());
    assert_eq!(got, vec![2, 4, 1, 3, 5]);
}

#[test]
fn order_by_two_keys() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items ORDER BY name ASC, price DESC"
    ).unwrap());
    // apple@4, apple@1, bread, cherry, date.
    assert_eq!(got, vec![4, 2, 1, 3, 5]);
}

#[test]
fn limit_without_order() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items LIMIT 3"
    ).unwrap());
    assert_eq!(got.len(), 3);
}

#[test]
fn order_by_then_limit() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = names(db.execute(
        "SELECT name FROM t.public.items ORDER BY name LIMIT 2"
    ).unwrap());
    assert_eq!(got, vec!["apple", "apple"]);
}

#[test]
fn order_by_then_limit_and_offset() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // SelectWindow grammar is LIMIT-before-OFFSET; the reverse
    // order remains unsupported (future ADR if needed).
    let got = names(db.execute(
        "SELECT name FROM t.public.items ORDER BY name LIMIT 2 OFFSET 2"
    ).unwrap());
    assert_eq!(got, vec!["bread", "cherry"]);
}

#[test]
fn order_by_composes_with_pk_where() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Base-engine WHERE only supports full-PK equality, so we
    // verify ORDER BY still parses after it.
    let got = ids(db.execute(
        "SELECT id FROM t.public.items WHERE id = 3 ORDER BY name"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn order_by_unknown_column_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items ORDER BY nope"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)));
}

#[test]
fn limit_exceeds_cap_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.items ORDER BY id LIMIT 999999"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn offset_past_end_returns_empty() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.items ORDER BY id OFFSET 100"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn nulls_last_default_for_asc() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))",
        "INSERT INTO t.public.items(id, note) VALUES(1, 'x')",
        "INSERT INTO t.public.items(id, note) VALUES(2, NULL)",
        "INSERT INTO t.public.items(id, note) VALUES(3, 'y')",
    ] { db.execute(sql).unwrap(); }
    let got = ids(db.execute(
        "SELECT id FROM t.public.items ORDER BY note"
    ).unwrap());
    assert_eq!(got, vec![1, 3, 2]);
}

#[test]
fn domain_engine_order_by_limit() {
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
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'c')")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(2, 'a')")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(3, 'b')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.items ORDER BY name LIMIT 2")? {
            SqlResultSet::Rows { rows, .. } => {
                let mut v = captured.lock().unwrap();
                for r in &rows {
                    if let Value::Int64(id) = r[0] { v.push(id); }
                }
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![2, 3]);
}

#[test]
fn mvcc_engine_order_by_desc() {
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
        s.execute("INSERT INTO t.public.items(id, name) VALUES(1, 'x')")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(2, 'y')")?;
        s.execute("INSERT INTO t.public.items(id, name) VALUES(3, 'z')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.items ORDER BY name DESC")? {
            SqlResultSet::Rows { rows, .. } => {
                let mut v = captured.lock().unwrap();
                for r in &rows {
                    if let Value::Int64(id) = r[0] { v.push(id); }
                }
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(*got.lock().unwrap(), vec![3, 2, 1]);
}
