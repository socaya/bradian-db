//! ADR 0178 — UNIQUE enforcement in `DomainSqlEngine` (shared-WAL
//! multi-table session engine).
//!
//! Scenarios:
//! - Duplicate INSERT within the same transaction is rejected.
//! - UPDATE that would collide with another row is rejected.
//! - Self-update (unique column to its own current value) is
//!   allowed (PK filter excludes the row from the iterator).
//! - UNIQUE on table A does not affect INSERTs into a sibling
//!   table B in the same domain.
//! - A rolled-back transaction leaves no trace: a subsequent
//!   INSERT of the same value in a fresh transaction succeeds.

use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed(root: &std::path::Path) {
    let ddl = SqlEngine::open(root).unwrap();
    for sql in [
        "CREATE DATABASE t",
        "CREATE SCHEMA t.public",
        "CREATE TABLE t.public.u (id BIGINT NOT NULL, email TEXT NOT NULL, note TEXT NOT NULL, UNIQUE (email), PRIMARY KEY(id))",
        "CREATE TABLE t.public.v (id BIGINT NOT NULL, label TEXT NOT NULL, PRIMARY KEY(id))",
    ] { ddl.execute(sql).unwrap(); }
}

fn open_domain(root: &std::path::Path) -> DomainSqlEngine {
    let dom = DomainSqlEngine::open(root).unwrap();
    dom.register_catalog_table("t.public.u").unwrap();
    dom.register_catalog_table("t.public.v").unwrap();
    dom
}

#[test]
fn duplicate_insert_in_shared_domain_is_rejected() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    let dom = open_domain(dir.path());
    let err = dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(2, 'a@x', 'two')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)),
        "expected SqlError::Unique; got {err:?}");
}

#[test]
fn update_collision_in_shared_domain_is_rejected() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    let dom = open_domain(dir.path());
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(2, 'b@x', 'two')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    let err = dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='b@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Unique(_)));
}

#[test]
fn self_update_in_shared_domain_is_allowed() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    let dom = open_domain(dir.path());
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE t.public.u SET email='a@x' WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn unique_on_one_table_does_not_affect_sibling_table_inserts() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    let dom = open_domain(dir.path());
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        // Two 'a@x' rows in t.public.v would be fine (no UNIQUE)
        // but we must respect PK uniqueness on id.
        s.execute("INSERT INTO t.public.v(id, label) VALUES(1, 'a@x')")?;
        s.execute("INSERT INTO t.public.v(id, label) VALUES(2, 'a@x')")?;
        // Meanwhile u has UNIQUE(email) — distinct emails ok.
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(10, 'c@x', 'ten')")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(11, 'd@x', 'eleven')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}

#[test]
fn rollback_leaves_no_visible_row_for_subsequent_unique_check() {
    let dir = tempdir().unwrap();
    seed(dir.path());
    let dom = open_domain(dir.path());
    // Insert + rollback — the row must not persist.
    let _ = dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("ROLLBACK")?;
        Ok(())
    });
    // Now insert the same unique value in a fresh transaction.
    dom.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("INSERT INTO t.public.u(id, email, note) VALUES(1, 'a@x', 'one')")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
}
