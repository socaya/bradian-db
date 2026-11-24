//! ADR 0204 — AND-chained JSON-path predicates in WHERE.

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
fn two_predicates_both_match() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' AND body ->> 'owner' = 'alice'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn two_predicates_one_matches_only() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // open status alone: rows 1, 2, 4.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' AND body -> 'age' > 28"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn three_predicates_chain() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' AND body -> 'age' >= 30 AND body -> 'age' < 40"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn and_is_logically_conjunctive() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // No row satisfies both.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' AND body ->> 'status' = 'closed'"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn and_with_container_form() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body #>> '{status}' = 'open' AND body #> '{age}' > 40"
    ).unwrap());
    assert_eq!(got, vec![2]);
}

#[test]
fn and_on_update_applies_to_both() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Update only alice/open row.
    db.execute(
        "UPDATE t.public.docs SET body -> 'owner' = 'ALICE' WHERE body ->> 'status' = 'open' AND body ->> 'owner' = 'alice'"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'owner' = 'ALICE'"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn and_on_delete_applies_to_both() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body ->> 'status' = 'open' AND body -> 'age' <= 30"
    ).unwrap();
    // Remaining ids: 2 (open, 42), 3 (closed, 35).
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open'").unwrap());
    assert_eq!(got, vec![2]);
    let got_closed = ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'status' = 'closed'").unwrap());
    assert_eq!(got_closed, vec![3]);
}

#[test]
fn json_path_and_bare_column_eq_mix_works() {
    // ADR 0246 — mixing a JSON-path predicate with a bare-column
    // `=` predicate is valid and returns rows matching both.
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open' AND id = 1"
    ).unwrap());
    assert_eq!(got, vec![1]);
}

#[test]
fn single_predicate_still_works() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'status' = 'open'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 4]);
}

#[test]
fn domain_engine_and_chain() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":1,"b":2}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"a":1,"b":3}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"a":2,"b":3}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'a' = 1 AND body -> 'b' = 3")? {
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
fn mvcc_engine_and_chain() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":1,"b":2}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"a":1,"b":3}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body -> 'a' = 1 AND body -> 'b' = 3")? {
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
