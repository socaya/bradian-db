//! ADR 0207 — IS NULL / IS NOT NULL on JSON path WHERE.

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
        // id=1 owner present; id=2 owner missing; id=3 owner JSON null.
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"owner":"alice","age":30}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"age":42}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"owner":null,"age":35}')"#,
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
fn is_null_matches_missing_and_explicit_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'owner' IS NULL"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn is_not_null_matches_present_non_null() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'owner' IS NOT NULL"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn is_null_with_text_terminator() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // ->> yields TEXT which is still NULL when the key is absent.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' IS NULL"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn is_null_in_and_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'owner' IS NULL AND body -> 'age' > 40"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn is_null_in_or_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' = 'alice' OR body -> 'owner' IS NULL"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn is_not_null_combined_with_not_in() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body -> 'owner' IS NOT NULL AND body ->> 'owner' NOT IN ('dave')"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn is_null_on_container_path() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body #> '{owner}' IS NULL"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn is_without_null_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'owner' IS 'alice'"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn is_null_on_update() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'owner' = 'anon' WHERE body -> 'owner' IS NULL"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' = 'anon'"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn is_null_on_delete() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body -> 'owner' IS NULL"
    ).unwrap();
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body -> 'age' > 0").unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn domain_engine_is_null() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"v"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'k' IS NULL")? {
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
fn mvcc_engine_is_not_null() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"v"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'k' IS NOT NULL")? {
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
