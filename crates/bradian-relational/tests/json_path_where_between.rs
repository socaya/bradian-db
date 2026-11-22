//! ADR 0208 — BETWEEN / NOT BETWEEN on JSON path WHERE.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice","age":20}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"name":"bob","age":30}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"name":"carol","age":40}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"name":"dave","age":50}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(5, '{"name":"eve","age":60}')"#,
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
fn between_numeric_inclusive() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' BETWEEN 30 AND 50"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn between_includes_boundaries() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' BETWEEN 20 AND 60"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3, 4, 5]);
}

#[test]
fn between_text_range() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Text lex range: 'b' .. 'd' → bob, carol.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' BETWEEN 'b' AND 'd'"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn not_between_inverts() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' NOT BETWEEN 30 AND 50"
    ).unwrap());
    assert_eq!(got, vec![1, 5]);
}

#[test]
fn between_in_and_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body -> 'age' BETWEEN 25 AND 55 AND body ->> 'name' <> 'dave'"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn between_in_or_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body -> 'age' BETWEEN 50 AND 60 OR body ->> 'name' = 'alice'"
    ).unwrap());
    assert_eq!(got, vec![1, 4, 5]);
}

#[test]
fn between_with_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body #> '{age}' BETWEEN 30 AND 40"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn between_empty_range_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' BETWEEN 50 AND 30"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn between_missing_and_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' BETWEEN 30 50"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn between_on_update() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'bracket' = 'mid' WHERE body -> 'age' BETWEEN 25 AND 45"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'bracket' = 'mid'"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn between_on_delete() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body -> 'age' NOT BETWEEN 30 AND 50"
    ).unwrap();
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body -> 'age' > 0").unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn between_composes_with_in() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body -> 'age' BETWEEN 25 AND 55 AND body ->> 'name' IN ('bob','dave')"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn domain_engine_between() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"n":10}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"n":20}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"n":30}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'n' BETWEEN 15 AND 25")? {
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

#[test]
fn mvcc_engine_not_between() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"n":10}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"n":50}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'n' NOT BETWEEN 20 AND 40")? {
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
    assert_eq!(*got.lock().unwrap(), vec![1, 2]);
}
