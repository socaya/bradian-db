//! ADR 0206 — IN / NOT IN on JSON path WHERE.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"status":"open","owner":"alice","age":30}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"status":"open","owner":"bob","age":42}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"status":"closed","owner":"alice","age":35}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"status":"open","owner":"carol","age":25}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(5, '{"status":"archived","owner":"dave","age":55}')"#,
    ] { db.execute(sql).unwrap(); }
    db
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
fn in_text_set() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IN ('alice','carol')"
    ).unwrap());
    assert_eq!(got, vec![1, 3, 4]);
}

#[test]
fn in_numeric_set() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' IN (30, 42, 55)"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 5]);
}

#[test]
fn in_single_value_matches_scalar() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IN ('bob')"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn not_in_excludes_values() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' NOT IN ('closed','archived')"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 4]);
}

#[test]
fn in_with_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body #>> '{owner}' IN ('alice','dave')"
    ).unwrap());
    assert_eq!(got, vec![1, 3, 5]);
}

#[test]
fn in_combined_with_and() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' IN ('alice','bob') AND body ->> 'status' = 'open'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn in_combined_with_or() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' IN ('carol') OR body ->> 'status' = 'archived'"
    ).unwrap());
    assert_eq!(got, vec![4, 5]);
}

#[test]
fn in_on_update() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'owner' = 'X' WHERE body ->> 'owner' IN ('alice','carol')"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' = 'X'"
    ).unwrap());
    assert_eq!(got, vec![1, 3, 4]);
}

#[test]
fn in_on_delete() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body ->> 'status' IN ('closed','archived')"
    ).unwrap();
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body -> 'age' > 0").unwrap());
    assert_eq!(got, vec![1, 2, 4]);
}

#[test]
fn in_empty_list_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IN ()"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn not_without_in_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' NOT 'alice'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn in_mixed_with_not_in_dnf() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // owner IN (alice, bob) AND status NOT IN (closed, archived)
    // → ids 1, 2.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' IN ('alice','bob') AND body ->> 'status' NOT IN ('closed','archived')"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn domain_engine_in() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"a"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"k":"b"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"k":"c"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 'k' IN ('a','c')")? {
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
    assert_eq!(*got.lock().unwrap(), vec![1, 3]);
}

#[test]
fn mvcc_engine_not_in() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"x"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"k":"y"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 'k' NOT IN ('y')")? {
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
    assert_eq!(*got.lock().unwrap(), vec![1]);
}
