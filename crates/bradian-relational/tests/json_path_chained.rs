//! ADR 0189 — chained JSON path segments across all four engines.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed_sql_engine(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":{"b":{"c":"deep"},"n":42}}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"a":{"b":{"c":42}}}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(3, '{"a":{"b":null}}')"#,
        r#"INSERT INTO t.public.docs(id, body) VALUES(4, '{"a":{}}')"#,
    ] { db.execute(sql).unwrap(); }
    db
}

#[test]
fn sql_engine_chained_arrow_returns_json() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("SELECT body -> 'a' -> 'b' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body->'a'->'b'".to_string()]);
            match &rows[0][0] {
                Value::Json(s) => {
                    let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                    assert_eq!(parsed["c"], "deep");
                }
                other => panic!("expected Json; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn sql_engine_chained_terminating_arrow_arrow_returns_text() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("SELECT body -> 'a' -> 'b' ->> 'c' FROM t.public.docs WHERE id=1").unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["body->'a'->'b'->>'c'".to_string()]);
            match &rows[0][0] {
                Value::String(s) => assert_eq!(s, "deep"),
                other => panic!("expected String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn sql_engine_chained_arrow_arrow_on_numeric_subvalue_renders_canonical_text() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute("SELECT body -> 'a' -> 'b' ->> 'c' FROM t.public.docs WHERE id=2").unwrap() {
        SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
            Value::String(s) => assert_eq!(s, "42"),
            other => panic!("expected String; got {other:?}"),
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn sql_engine_chained_path_short_circuits_on_null_midway() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // body.a.b is JSON null — anything below returns Null.
    match db.execute("SELECT body -> 'a' -> 'b' ->> 'c' FROM t.public.docs WHERE id=3").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn sql_engine_chained_path_missing_key_returns_null() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // body.a.b does not exist on id=4.
    match db.execute("SELECT body -> 'a' -> 'b' FROM t.public.docs WHERE id=4").unwrap() {
        SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn chained_arrow_arrow_followed_by_more_segments_is_rejected() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    // `->>` can only appear last; parser must reject.
    let err = db.execute("SELECT body ->> 'a' -> 'b' FROM t.public.docs WHERE id=1").unwrap_err();
    assert!(matches!(err, SqlError::Unsupported(_)),
        "`->>` followed by more segments must be rejected; got {err:?}");
}

#[test]
fn chained_path_mixes_with_bare_column_projection() {
    let dir = tempdir().unwrap();
    let db = seed_sql_engine(dir.path());
    match db.execute(
        "SELECT id, body -> 'a' ->> 'n' FROM t.public.docs WHERE id=1"
    ).unwrap() {
        SqlResultSet::Rows { columns, rows } => {
            assert_eq!(columns, vec!["id".to_string(), "body->'a'->>'n'".to_string()]);
            match &rows[0][1] {
                Value::String(s) => assert_eq!(s, "42"),
                other => panic!("expected String; got {other:?}"),
            }
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

// ---- cross-engine confirmations ----

#[test]
fn domain_engine_chained_path() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":{"b":"deep"}}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body -> 'a' ->> 'b' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::String(ref s_) => assert_eq!(s_, "deep"),
                other => panic!("expected String; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_engine_chained_path() {
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
            s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":{"b":{"c":"ok"}}}')"#)?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body -> 'a' -> 'b' ->> 'c' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::String(ref v) => assert_eq!(v, "ok"),
                other => panic!("expected String; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_engine_chained_path() {
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
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"a":{"b":{"c":"v"}}}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body -> 'a' -> 'b' -> 'c' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(ref sj) => {
                    assert_eq!(sj, "\"v\"");
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
