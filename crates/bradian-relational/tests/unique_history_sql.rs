//! ADR 0179 — UNIQUE enforcement in `HistorySqlEngine` INSERT + UPDATE.
//!
//! Uses the legacy-shared → history-domain migration path to open a
//! HistorySqlEngine with UNIQUE declared on the seeded tables.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn open_history_with_unique() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    {
        let ddl = SqlEngine::open(&source).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.u (id BIGINT NOT NULL, email TEXT NOT NULL, note TEXT NOT NULL, UNIQUE (email), PRIMARY KEY(id))",
        ] { ddl.execute(sql).unwrap(); }
    }
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.u").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
            s.execute("INSERT INTO t.public.u(id, email, note) VALUES(2, 'b@x', 'two')")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    (dir, history)
}

#[test]
fn history_duplicate_insert_is_rejected() {
    let (_dir, history) = open_history_with_unique();
    let db = HistorySqlEngine::open(&history).unwrap();
    let err = db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(3, 'a@x', 'three')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)),
        "expected SqlError::Unique; got {err:?}");
}

#[test]
fn history_update_collision_is_rejected() {
    let (_dir, history) = open_history_with_unique();
    let db = HistorySqlEngine::open(&history).unwrap();
    let err = db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='b@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}

#[test]
fn history_self_update_is_allowed() {
    let (_dir, history) = open_history_with_unique();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='a@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_insert_distinct_unique_value_succeeds() {
    let (_dir, history) = open_history_with_unique();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(3, 'c@x', 'three')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn history_update_to_brand_new_unique_value_succeeds() {
    let (_dir, history) = open_history_with_unique();
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='z@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}
