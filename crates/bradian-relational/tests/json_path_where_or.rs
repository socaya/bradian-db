//! ADR 0205 — OR / DNF combinators in JSON-path WHERE.

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
fn or_matches_either_branch() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' = 'bob' OR body ->> 'owner' = 'carol'"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn or_is_disjunctive_not_conjunctive() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // age >= 40 → id 2; owner = alice → ids 1, 3. Union: 1, 2, 3.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' >= 40 OR body ->> 'owner' = 'alice'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3]);
}

#[test]
fn and_binds_tighter_than_or() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // (status=open AND owner=alice) OR (status=closed)
    // → ids 1, 3.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'status' = 'open' AND body ->> 'owner' = 'alice' \
            OR body ->> 'status' = 'closed'"
    ).unwrap());
    assert_eq!(got, vec![1, 3]);
}

#[test]
fn three_or_groups() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' = 'bob' \
            OR body ->> 'owner' = 'carol' \
            OR body ->> 'status' = 'closed'"
    ).unwrap());
    assert_eq!(got, vec![2, 3, 4]);
}

#[test]
fn or_does_not_duplicate_rows() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Row 1 matches both branches but must appear once only.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'owner' = 'alice' OR body ->> 'status' = 'open'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 3, 4]);
}

#[test]
fn or_with_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body #> '{age}' > 40 OR body #>> '{owner}' = 'carol'"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn or_on_update_applies_to_union() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'owner' = 'X' WHERE body ->> 'owner' = 'bob' OR body ->> 'owner' = 'carol'"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' = 'X'"
    ).unwrap());
    assert_eq!(got, vec![2, 4]);
}

#[test]
fn or_on_delete_applies_to_union() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body -> 'age' <= 25 OR body ->> 'status' = 'closed'"
    ).unwrap();
    // Removed: id 3 (closed), id 4 (age 25). Left: 1, 2.
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open'").unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn or_followed_by_pk_rejected() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' OR id = 1"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)));
}

#[test]
fn and_or_mixing_follows_dnf() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // A AND B OR C AND D → (A AND B) OR (C AND D).
    // (status=open AND age>=40) OR (owner=alice AND status=closed)
    // → ids 2, 3.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'status' = 'open' AND body -> 'age' >= 40 \
            OR body ->> 'owner' = 'alice' AND body ->> 'status' = 'closed'"
    ).unwrap());
    assert_eq!(got, vec![2, 3]);
}

#[test]
fn domain_engine_or_chain() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":1}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"a":2}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"a":3}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'a' = 1 OR body -> 'a' = 3")? {
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
fn mvcc_engine_or_chain() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":1}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"a":2}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'a' = 1 OR body -> 'a' = 2")? {
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
