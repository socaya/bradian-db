//! ADR 0210 — ILIKE / NOT ILIKE (case-insensitive LIKE).

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"Alice"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"name":"alison"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"name":"BOB"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"name":"CAROL"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(5, '{"name":"carol"}')"#,
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
fn ilike_matches_mixed_case() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' ILIKE 'al%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn ilike_matches_upper_pattern() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' ILIKE 'CAR%'"
    ).unwrap());
    assert_eq!(got, vec![4, 5]);
}

#[test]
fn ilike_underscore_wildcard() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' ILIKE 'b_b'"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn not_ilike_inverts() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' NOT ILIKE 'al%'"
    ).unwrap());
    assert_eq!(got, vec![3, 4, 5]);
}

#[test]
fn ilike_on_arrow_json_string() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'name' ILIKE 'AL%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn ilike_composes_with_and() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'name' ILIKE 'C%' AND body ->> 'name' NOT ILIKE 'CAROL'"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn ilike_composes_with_or() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'name' ILIKE 'b%' OR body ->> 'name' ILIKE 'a%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn ilike_requires_text_pattern() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' ILIKE 123"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn ilike_on_update() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'cohort' = 'a' WHERE body ->> 'name' ILIKE 'A%'"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'cohort' = 'a'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn ilike_on_delete() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body ->> 'name' ILIKE 'C%'"
    ).unwrap();
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' IS NOT NULL").unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn domain_engine_ilike() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"s":"FOO"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"s":"bar"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 's' ILIKE 'foo'")? {
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

#[test]
fn mvcc_engine_not_ilike() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"s":"ABC"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"s":"xyz"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 's' NOT ILIKE 'a%'")? {
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
    assert_eq!(*got.lock().unwrap(), vec![2]);
}
