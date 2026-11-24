//! ADR 0281 — Role catalog.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    (dir, db)
}

#[test]
fn create_role_with_caps() {
    let (_dir, db) = seed();
    db.execute("CREATE ROLE reader WITH READ").unwrap();
    let snap = db.user_catalog_snapshot();
    let role = snap.role("reader").unwrap();
    assert!(role.capabilities.contains(&bradian_core::Capability::Read));
}

#[test]
fn grant_role_gives_user_role_caps() {
    let (_dir, db) = seed();
    db.execute("CREATE ROLE reader WITH READ").unwrap();
    db.execute("GRANT ROLE reader TO bob").unwrap();
    // bob doesn't have global Read in his own caps but through
    // reader role he does — execute SELECT succeeds.
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("SELECT id FROM t.public.items")).unwrap();
}

#[test]
fn revoke_role_removes_access() {
    let (_dir, db) = seed();
    db.execute("CREATE ROLE reader WITH READ").unwrap();
    db.execute("GRANT ROLE reader TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("SELECT id FROM t.public.items")).unwrap();
    db.execute("REVOKE ROLE reader FROM bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("SELECT id FROM t.public.items")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn drop_role_revokes_from_everyone() {
    let (_dir, db) = seed();
    db.execute("CREATE ROLE reader WITH READ").unwrap();
    db.execute("GRANT ROLE reader TO bob").unwrap();
    db.execute("DROP ROLE reader").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("SELECT id FROM t.public.items")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
    // The role is also gone.
    let snap = db.user_catalog_snapshot();
    assert!(snap.role("reader").is_none());
}

#[test]
fn grant_unknown_role_rejected() {
    let (_dir, db) = seed();
    let err = db.execute("GRANT ROLE ghost TO bob").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn drop_unknown_role_is_idempotent() {
    let (_dir, db) = seed();
    db.execute("DROP ROLE ghost").unwrap();
}

#[test]
fn role_capabilities_persist_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE DATABASE t").unwrap();
        db.execute("CREATE SCHEMA t.public").unwrap();
        db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
        db.execute("CREATE ROLE reader WITH READ").unwrap();
        db.execute("GRANT ROLE reader TO bob").unwrap();
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("SELECT id FROM t.public.items")).unwrap();
}

#[test]
fn multi_cap_role() {
    let (_dir, db) = seed();
    db.execute("CREATE ROLE rw WITH READ, WRITE").unwrap();
    db.execute("GRANT ROLE rw TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s| {
        s.execute("SELECT id FROM t.public.items")?;
        s.execute("INSERT INTO t.public.items(id) VALUES(1)")?;
        Ok(())
    }).unwrap();
}
