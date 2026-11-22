//! ADR 0193 — JSON path mutation on HistorySqlEngine UPDATE.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn seed_history() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.docs (id BIGINT NOT NULL, amount BIGINT NOT NULL, body JSON NOT NULL, PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.docs").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute(r#"INSERT INTO t.public.docs(id, amount, body) VALUES(1, 10, '{"k":1,"name":"alice"}')"#)?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    (dir, history)
}

fn body_of(db: &HistorySqlEngine, id: i64) -> serde_json::Value {
    let out = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
    let captured = out.clone();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        if let SqlResultSet::Rows { rows, .. } =
            s.execute(&format!("SELECT body FROM t.public.docs WHERE id={id}"))?
        {
            match &rows[0][0] {
                Value::Json(j) => *captured.lock().unwrap() = serde_json::from_str(j).unwrap(),
                Value::String(j) => *captured.lock().unwrap() = serde_json::from_str(j).unwrap(),
                other => panic!("expected json cell; got {other:?}"),
            }
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    let guard = out.lock().unwrap();
    guard.clone()
}

#[test]
fn history_set_arrow_key_replaces_scalar_subvalue() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.docs SET body -> 'name' = 'bob' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let body = body_of(&db, 1);
    assert_eq!(body["name"], "bob");
    assert_eq!(body["k"], 1);
}

#[test]
fn history_mixes_expression_and_path_mutation_in_one_set() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    // Expression RHS for amount (numeric) AND JSON path for body
    // in a single UPDATE SET.
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.docs SET amount = amount + 5, body -> 'k' = 99 WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT amount FROM t.public.docs WHERE id=1")? {
            SqlResultSet::Rows { rows, .. } => assert_eq!(rows[0][0], Value::Int64(15)),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
    assert_eq!(body_of(&db, 1)["k"], 99);
}

#[test]
fn history_chained_path_auto_creates_intermediate() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.docs SET body -> 'meta' -> 'origin' = 'test' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let body = body_of(&db, 1);
    assert_eq!(body["meta"]["origin"], "test");
}

#[test]
fn history_arrow_arrow_writes_json_string() {
    let (_dir, history) = seed_history();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.docs SET body ->> 'k' = 'nine' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(body_of(&db, 1)["k"], "nine");
}
