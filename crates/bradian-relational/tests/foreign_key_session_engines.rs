//! ADR 0187 — FK enforcement in `DomainSqlEngine`, `HistorySqlEngine`
//! and `MvccSqlEngine`. Mirrors ADR 0184/0185 from `SqlEngine`.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::history_migration::migrate_shared_to_history;
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::mvcc_sql::MvccSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn ddl(root: &std::path::Path, extra_action: &str) {
    let db = SqlEngine::open(root).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.parents (id BIGINT NOT NULL, PRIMARY KEY(id))",
    ] { db.execute(sql).unwrap(); }
    let create_child = format!(
        "CREATE TABLE t.public.children (id BIGINT NOT NULL, parent_id BIGINT{nullable}, FOREIGN KEY (parent_id) REFERENCES t.public.parents (id){extra_action}, PRIMARY KEY(id))",
        nullable = if extra_action.contains("SET NULL") { "" } else { " NOT NULL" },
        extra_action = extra_action,
    );
    db.execute(&create_child).unwrap();
}

// ---- DomainSqlEngine ----

#[test]
fn domain_insert_missing_parent_is_rejected() {
    let dir = tempdir().unwrap();
    ddl(dir.path(), "");
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.parents").unwrap();
    dom.register_catalog_table("t.public.children").unwrap();
    let err = dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 99)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "missing-parent"));
}

#[test]
fn domain_insert_sees_pending_parent_in_same_tx() {
    let dir = tempdir().unwrap();
    ddl(dir.path(), "");
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.parents").unwrap();
    dom.register_catalog_table("t.public.children").unwrap();
    // Insert parent and child in the same transaction — FK
    // enforcement reads through tx.scan so the pending parent
    // is visible before COMMIT.
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn domain_parent_delete_restrict() {
    let dir = tempdir().unwrap();
    ddl(dir.path(), "");
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.parents").unwrap();
    dom.register_catalog_table("t.public.children").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let err = dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}

#[test]
fn domain_parent_delete_cascade_removes_children() {
    let dir = tempdir().unwrap();
    ddl(dir.path(), " ON DELETE CASCADE");
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.parents").unwrap();
    dom.register_catalog_table("t.public.children").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.children")? {
            SqlResultSet::Rows { rows, .. } => assert!(rows.is_empty()),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn domain_parent_delete_setnull() {
    let dir = tempdir().unwrap();
    ddl(dir.path(), " ON DELETE SET NULL");
    let dom = DomainSqlEngine::open(dir.path()).unwrap();
    dom.register_catalog_table("t.public.parents").unwrap();
    dom.register_catalog_table("t.public.children").unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT parent_id FROM t.public.children WHERE id=10")? {
            SqlResultSet::Rows { rows, .. } => assert_eq!(rows, vec![vec![Value::Null]]),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

// ---- HistorySqlEngine ----

fn seed_history(action: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempdir().unwrap();
    let source = dir.path().join("src");
    let history = dir.path().join("hist");
    ddl(&source, action);
    {
        let dom = DomainSqlEngine::open(&source).unwrap();
        dom.register_catalog_table("t.public.parents").unwrap();
        dom.register_catalog_table("t.public.children").unwrap();
        dom.with_session(|s| {
            s.execute("BEGIN")?;
            s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
            s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
            s.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    migrate_shared_to_history(&source, &history).unwrap();
    (dir, history)
}

#[test]
fn history_insert_missing_parent_is_rejected() {
    let (_dir, history) = seed_history("");
    let db = HistorySqlEngine::open(&history).unwrap();
    let err = db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(11, 99)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "missing-parent"));
}

#[test]
fn history_parent_delete_restrict() {
    let (_dir, history) = seed_history("");
    let db = HistorySqlEngine::open(&history).unwrap();
    let err = db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}

#[test]
fn history_parent_delete_cascade() {
    let (_dir, history) = seed_history(" ON DELETE CASCADE");
    let db = HistorySqlEngine::open(&history).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.children")? {
            SqlResultSet::Rows { rows, .. } => assert!(rows.is_empty()),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

// ---- MvccSqlEngine ----

fn mvcc_setup(root: &std::path::Path, action: &str) {
    ddl(root, action);
    let dom = DomainSqlEngine::open(root).unwrap();
    dom.register_catalog_table("t.public.parents").unwrap();
    dom.register_catalog_table("t.public.children").unwrap();
}

#[test]
fn mvcc_insert_missing_parent_is_rejected() {
    let dir = tempdir().unwrap();
    mvcc_setup(dir.path(), "");
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    let err = engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 99)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "missing-parent"));
}

#[test]
fn mvcc_parent_delete_restrict() {
    let dir = tempdir().unwrap();
    mvcc_setup(dir.path(), "");
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let err = engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::ForeignKey { reason, .. } if reason == "restrict"));
}

#[test]
fn mvcc_parent_delete_cascade() {
    let dir = tempdir().unwrap();
    mvcc_setup(dir.path(), " ON DELETE CASCADE");
    let engine = MvccSqlEngine::open(dir.path()).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.parents(id) VALUES(1)")?;
        s.execute("INSERT INTO t.public.children(id, parent_id) VALUES(10, 1)")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("DELETE FROM t.public.parents WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    engine.with_session(|s| {
        s.execute("BEGIN")?;
        match s.execute("SELECT id FROM t.public.children")? {
            SqlResultSet::Rows { rows, .. } => assert!(rows.is_empty()),
            other => panic!("expected Rows; got {other:?}"),
        }
        s.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}
