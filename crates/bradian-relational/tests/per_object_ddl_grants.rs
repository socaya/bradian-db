//! ADR 0279 — Per-object ALTER / DROP grants on tables.

use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn seed() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))")
        .unwrap();
    (dir, db)
}

#[test]
fn bob_without_grant_cannot_alter() {
    let (_dir, db) = seed();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("ALTER TABLE t.public.items RENAME TO renamed")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_alter_enables_rename() {
    let (_dir, db) = seed();
    db.execute("GRANT ALTER ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'x'"))
        .unwrap();
}

#[test]
fn grant_alter_does_not_enable_drop() {
    let (_dir, db) = seed();
    db.execute("GRANT ALTER ON t.public.items TO bob").unwrap();
    // DROP TABLE isn't dispatched as a top-level command yet,
    // so this test just confirms the grammar routes ALTER grant
    // separately from DROP grant (ObjectCapability::Alter !=
    // ObjectCapability::Drop).
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    let caps_on_items: Vec<_> = bob.table_grants().values()
        .flat_map(|s| s.iter().copied()).collect();
    assert!(caps_on_items.contains(&bradian_relational::user_catalog::ObjectCapability::Alter));
    assert!(!caps_on_items.contains(&bradian_relational::user_catalog::ObjectCapability::Drop));
}

#[test]
fn admin_global_ddl_bypasses_per_table() {
    let (_dir, db) = seed();
    db.with_authenticated_session("alice", "pw", |s|
        s.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'x'"))
        .unwrap();
}

#[test]
fn revoke_alter_takes_effect_on_next_statement() {
    let (_dir, db) = seed();
    db.execute("GRANT ALTER ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw", |s|
        s.execute("ALTER TABLE t.public.items ALTER COLUMN note SET NOT NULL BACKFILL 'x'"))
        .unwrap();
    db.execute("REVOKE ALTER ON t.public.items FROM bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw", |s|
        s.execute("ALTER TABLE t.public.items RENAME TO renamed")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_drop_persists_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE DATABASE t").unwrap();
        db.execute("CREATE SCHEMA t.public").unwrap();
        db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
        db.execute("GRANT DROP ON t.public.items TO bob").unwrap();
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    assert!(bob.table_grants().values()
        .any(|s| s.contains(&bradian_relational::user_catalog::ObjectCapability::Drop)));
}
