//! ADR 0190 — WHERE-side JSON path equality predicates in SELECT.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed_sql_engine(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice","level":3}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"name":"bob","level":5}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"name":"carol","level":3}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"name":"derek"}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn where_arrow_arrow_text_equality() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("SELECT id FROM t.public.docs WHERE body ->> 'name' = 'bob'").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(2));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_arrow_arrow_matches_numeric_subvalue_as_text() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("SELECT id FROM t.public.docs WHERE body ->> 'level' = '3'").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            let ids: Vec<Value> = rows.into_iter().map(|r| r[0].clone()).collect();
            assert_eq!(ids, vec![Value::Int64(1), Value::Int64(3)]);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_missing_key_never_matches() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // id=4 has no level. Filter by level='3' must not match it.
    match db.execute("SELECT id FROM t.public.docs WHERE body ->> 'level' = '3'").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            let ids: Vec<Value> = rows.into_iter().map(|r| r[0].clone()).collect();
            assert!(!ids.contains(&Value::Int64(4)));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_combines_with_json_path_projection() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // Project the JSON path AND filter by a JSON path.
    match db.execute(
        "SELECT id, body ->> 'name' FROM t.public.docs WHERE body ->> 'level' = '3'"
    ).unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["id".to_string(), "body->>'name'".to_string()]);
            let names: Vec<String> = rows.iter().map(|r| match &r[1] {
                Value::String(s) => s.clone(),
                _ => String::new(),
            }).collect();
            assert_eq!(names, vec!["alice", "carol"]);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_chained_arrow_path_filter() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":{"b":"x"}}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"a":{"b":"y"}}')"#,
    ] { db.execute(sql).unwrap(); }
    match db.execute(
        "SELECT id FROM t.public.docs WHERE body -> 'a' ->> 'b' = 'x'"
    ).unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(1));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn where_pk_equality_still_works() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // Bare equality path must take tx.get fast path.
    match db.execute("SELECT id FROM t.public.docs WHERE id = 2").unwrap() {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0][0], Value::Int64(2));
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

// ---- session engines ----

#[test]
fn domain_where_json_path_filter() {
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
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 'k' = 'b'")? {
            SqlResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], Value::Int64(2));
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_where_json_path_filter() {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":"a"}')"#)?;
            s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"k":"b"}')"#)?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 'k' = 'a'")? {
            SqlResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], Value::Int64(1));
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_where_json_path_filter() {
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
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.docs WHERE body ->> 'k' = 'y'")? {
            SqlResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], Value::Int64(2));
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
