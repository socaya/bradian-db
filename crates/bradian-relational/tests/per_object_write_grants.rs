//! ADR 0278 — GRANT INSERT / UPDATE / DELETE ON <table>.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn admin_plus_bob() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))")
        .unwrap();
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    (dir, db)
}

#[test]
fn bob_without_grant_cannot_insert() {
    let (_dir, db) = admin_plus_bob();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("INSERT INTO t.public.items(id, note) VALUES(2, 'x')")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_insert_allows_bob_to_insert() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT INSERT ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("INSERT INTO t.public.items(id, note) VALUES(2, 'x')")).unwrap();
}

#[test]
fn grant_insert_does_not_enable_update() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT INSERT ON t.public.items TO bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("UPDATE t.public.items SET note = 'y' WHERE id = 1")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_update_allows_bob_to_update() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT UPDATE ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("UPDATE t.public.items SET note = 'y' WHERE id = 1")).unwrap();
}

#[test]
fn grant_delete_allows_bob_to_delete() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT DELETE ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("DELETE FROM t.public.items WHERE id = 1")).unwrap();
}

#[test]
fn revoke_insert_takes_effect_after_session_reopen() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT INSERT ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("INSERT INTO t.public.items(id, note) VALUES(2, 'x')")).unwrap();
    db.execute("REVOKE INSERT ON t.public.items FROM bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("INSERT INTO t.public.items(id, note) VALUES(3, 'y')")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn admin_with_global_write_bypasses_per_table_grant() {
    let (_dir, db) = admin_plus_bob();
    // alice is admin.
    db.with_authenticated_session("alice", "pw", |s|
        s.execute("INSERT INTO t.public.items(id, note) VALUES(99, 'z')")).unwrap();
}

#[test]
fn snapshot_shows_insert_grant() {
    use bradian_core::Capability;
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT INSERT ON t.public.items TO bob").unwrap();
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    assert_eq!(bob.table_grants().len(), 1);
    // bob does NOT have global Write.
    assert!(!bob.capabilities().contains(&Capability::Write));
}
