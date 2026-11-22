//! ADR 0209 — LIKE / NOT LIKE on JSON path WHERE.

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
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"name":"alison"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"name":"bob"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"name":"carol"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(5, '{"name":"CAROL"}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(6, '{"name":"albert"}')"#,
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
fn like_prefix_percent() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE 'al%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 6]);
}

#[test]
fn like_suffix_percent() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE '%ol'"
    ).unwrap());
    assert_eq!(got, vec![4]);
}

#[test]
fn like_both_sides_percent() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE '%ar%'"
    ).unwrap());
    assert_eq!(got, vec![4]);
}

#[test]
fn like_underscore_single_char() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // Three chars starting with b: bob only.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE 'b_b'"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn like_mixed_percent_and_underscore() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // 'a_i%' → ali*, matches alice and alison.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE 'a_i%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn like_is_case_sensitive() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE 'CA%'"
    ).unwrap());
    assert_eq!(got, vec![5]);
}

#[test]
fn like_exact_match_without_wildcards() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE 'bob'"
    ).unwrap());
    assert_eq!(got, vec![3]);
}

#[test]
fn not_like_inverts() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' NOT LIKE 'al%'"
    ).unwrap());
    assert_eq!(got, vec![3, 4, 5]);
}

#[test]
fn like_on_arrow_json_string_works() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    // `->` yields a Value::Json("\"alice\"") on string leaves;
    // LIKE unwraps the JSON string before matching.
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'name' LIKE 'ali%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn like_on_non_string_returns_false() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"age":30}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"age":40}')"#,
    ] { db.execute(sql).unwrap(); }
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'age' LIKE '3%'"
    ).unwrap());
    assert!(got.is_empty());
}

#[test]
fn like_composes_with_and() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'name' LIKE 'a%' AND body ->> 'name' NOT LIKE '%ert'"
    ).unwrap());
    assert_eq!(got, vec![1, 2]);
}

#[test]
fn like_composes_with_or() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs \
         WHERE body ->> 'name' LIKE 'al%' OR body ->> 'name' LIKE 'c%'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 4, 6]);
}

#[test]
fn like_requires_text_pattern() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    let err = db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'name' LIKE 123"
    ).unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)));
}

#[test]
fn like_on_update() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "UPDATE t.public.docs SET body -> 'cohort' = 'al' WHERE body ->> 'name' LIKE 'al%'"
    ).unwrap();
    let got = ids(db.execute(
        "SELECT id FROM t.public.docs WHERE body ->> 'cohort' = 'al'"
    ).unwrap());
    assert_eq!(got, vec![1, 2, 6]);
}

#[test]
fn like_on_delete() {
    let dir = tempdir().unwrap();
    let db = seed(dir.path());
    db.execute(
        "DELETE FROM t.public.docs WHERE body ->> 'name' NOT LIKE 'al%'"
    ).unwrap();
    let got = ids(db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' IS NOT NULL").unwrap());
    assert_eq!(got, vec![1, 2, 6]);
}

#[test]
fn domain_engine_like() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"s":"foo"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"s":"bar"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"s":"foobar"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    dom.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 's' LIKE 'foo%'")? {
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
fn mvcc_engine_not_like() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"s":"abc"}')"#)?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"s":"xyz"}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<i64>::new()));
    let captured = got.clone();
    engine.with_session(move |s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 's' NOT LIKE 'a%'")? {
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
