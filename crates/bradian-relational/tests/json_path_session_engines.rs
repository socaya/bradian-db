//! ADR 0188 — JSON path operators (`->`, `->>`) in the three
//! session engines' SELECT. Mirrors ADR 0186 from SqlEngine.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn ddl(root: &std::path::Path) {
    let db = SqlEngine::open(root).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.docs (id BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
}

// ---- DomainSqlEngine ----

#[test]
fn domain_arrow_extracts_json_subvalue() {
    let dir = tempdir().unwrap();
    ddl(dir.path());
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.docs").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice","level":3}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body ->> 'name' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { columns, rows, .. } => {
                assert_eq!(columns, vec!["body->>'name'".to_string()]);
                match &rows[0][0] {
                    Value::String(ref v) => assert_eq!(v, "alice"),
                    other => panic!("expected String; got {other:?}"),
                }
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn domain_arrow_returns_json_subvalue() {
    let dir = tempdir().unwrap();
    ddl(dir.path());
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.docs").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"nested":{"k":42}}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body -> 'nested' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => match &rows[0][0] {
                Value::Json(s) => {
                    let parsed: serde_json::Value = serde_json::from_str(s).unwrap();
                    assert_eq!(parsed["k"], 42);
                }
                other => panic!("expected Json; got {other:?}"),
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

// ---- HistorySqlEngine ----

fn seeded_history() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    ddl(&source);
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"alice","level":3}')"#)?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    (dir, history)
}

#[test]
fn history_arrow_extracts_json_subvalue() {
    let (_dir, history) = seeded_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body ->> 'name' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { columns, rows, .. } => {
                assert_eq!(columns, vec!["body->>'name'".to_string()]);
                match &rows[0][0] {
                    Value::String(ref v) => assert_eq!(v, "alice"),
                    other => panic!("expected String; got {other:?}"),
                }
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_arrow_combines_with_order_by() {
    let (_dir, history) = seeded_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    // Add a second row so ordering is visible.
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(2, '{"name":"bob","level":9}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id, body ->> 'name' FROM t.public.docs ORDER BY id")? {
            SqlResultSet::Rows { rows, .. } => {
                let names: Vec<String> = rows.iter().map(|r| match &r[1] {
                    Value::String(ref v) => v.clone(),
                    _ => String::new(),
                }).collect();
                assert_eq!(names, vec!["alice", "bob"]);
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

// ---- MvccSqlEngine ----

#[test]
fn mvcc_arrow_extracts_json_subvalue() {
    let dir = tempdir().unwrap();
    ddl(dir.path());
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"name":"carol","level":7}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body ->> 'name' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { columns, rows, .. } => {
                assert_eq!(columns, vec!["body->>'name'".to_string()]);
                match &rows[0][0] {
                    Value::String(ref v) => assert_eq!(v, "carol"),
                    other => panic!("expected String; got {other:?}"),
                }
            }
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn mvcc_arrow_missing_key_returns_null() {
    let dir = tempdir().unwrap();
    ddl(dir.path());
    {
        let dom = DomainSqlEngine::open(dir.path()).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
    }
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute(r#"INSERT INTO t.public.docs(id, body) VALUES(1, '{"k":1}')"#)?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT body -> 'nope' FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Null),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
