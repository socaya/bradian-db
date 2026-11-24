//! ADR 0251 — GRANT / REVOKE grammar + first-user bootstrap.

use bradian_core::Capability;
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed_admin_then_bob(dir: &std::path::Path) -> SqlEngine {
    let db = SqlEngine::open(dir).unwrap();
    // First user created in an empty catalog is bootstrapped admin.
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    // Subsequent users start with no capabilities.
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db
}

#[test]
fn first_user_bootstraps_admin() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    let snap = db.user_catalog_snapshot();
    let alice = snap.get("alice").unwrap();
    assert!(alice.capabilities().contains(&Capability::Read));
    assert!(alice.capabilities().contains(&Capability::Write));
    assert!(alice.capabilities().contains(&Capability::Ddl));
    assert!(alice.capabilities().contains(&Capability::Admin));
}

#[test]
fn second_user_starts_empty() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    assert!(bob.capabilities().is_empty());
}

#[test]
fn empty_cap_session_cannot_run_sql() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    let err = db.with_authenticated_session("bob", "pw", |s| s.execute("SELECT 1"))
        .unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_read_enables_select() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    db.execute("GRANT READ TO bob").unwrap();
    // bob can now run SELECT. The SELECT itself fails at
    // resolution (no table), but authz is NOT the reason.
    let err = db.with_authenticated_session("bob", "pw", |s| s.execute("SELECT id FROM t.public.items"))
        .unwrap_err();
    assert!(!matches!(err, SqlError::Authz(_)), "unexpectedly got authz, {err:?}");
}

#[test]
fn grant_write_without_ddl_blocks_create() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    db.execute("GRANT READ TO bob").unwrap();
    db.execute("GRANT WRITE TO bob").unwrap();
    // bob still lacks DDL — CREATE DATABASE must fail with authz.
    let err = db.with_authenticated_session("bob", "pw", |s| s.execute("CREATE DATABASE t"))
        .unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_ddl_enables_create() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    db.execute("GRANT DDL TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s| {
        s.execute("CREATE DATABASE t")?;
        Ok(())
    }).unwrap();
}

#[test]
fn revoke_removes_capability() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    db.execute("GRANT READ TO bob").unwrap();
    db.execute("REVOKE READ FROM bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s| s.execute("SELECT 1"))
        .unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_idempotent() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    db.execute("GRANT READ TO bob").unwrap();
    db.execute("GRANT READ TO bob").unwrap();
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    assert_eq!(bob.capabilities().len(), 1);
    assert!(bob.capabilities().contains(&Capability::Read));
}

#[test]
fn revoke_absent_capability_is_idempotent() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    // bob never had WRITE; revoking it should not error.
    db.execute("REVOKE WRITE FROM bob").unwrap();
}

#[test]
fn grant_unknown_user_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    let err = db.execute("GRANT READ TO ghost").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn grant_unknown_capability_rejected() {
    let dir = tempdir().unwrap();
    let db = seed_admin_then_bob(dir.path());
    let err = db.execute("GRANT RUMBA TO bob").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn capabilities_persist_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = seed_admin_then_bob(dir.path());
        db.execute("GRANT READ TO bob").unwrap();
        db.execute("GRANT WRITE TO bob").unwrap();
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    assert!(bob.capabilities().contains(&Capability::Read));
    assert!(bob.capabilities().contains(&Capability::Write));
    assert!(!bob.capabilities().contains(&Capability::Ddl));
}

#[test]
fn bootstrap_only_applies_to_first_user_even_after_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    }
    // Reopen — alice is still there. Creating a second user
    // must NOT auto-admin them.
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    let snap = db.user_catalog_snapshot();
    assert!(snap.get("alice").unwrap().capabilities().contains(&Capability::Admin));
    assert!(snap.get("bob").unwrap().capabilities().is_empty());
}

#[test]
fn drop_all_users_then_create_admin_bootstraps_again() {
    // ADR 0251 — the bootstrap decision looks at whether the
    // live catalog is empty at CREATE time. Dropping every
    // existing user restores the bootstrap privilege to the
    // next CREATE USER.
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("DROP USER alice").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    let snap = db.user_catalog_snapshot();
    assert!(snap.get("bob").unwrap().capabilities().contains(&Capability::Admin));
}
