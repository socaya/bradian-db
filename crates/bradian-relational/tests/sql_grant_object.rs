//! ADR 0258 — GRANT SELECT ON <table> TO <user>.

use bradian_relational::sql::{SqlEngine, SqlError, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn admin_plus_bob() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    // First user bootstraps admin.
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE DATABASE t").unwrap();
    db.execute("CREATE SCHEMA t.public").unwrap();
    db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))")
        .unwrap();
    db.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'a')").unwrap();
    db.execute("CREATE TABLE t.public.other (id BIGINT NOT NULL, PRIMARY KEY(id))").unwrap();
    (dir, db)
}

#[test]
fn bob_without_grant_cannot_select() {
    let (_dir, db) = admin_plus_bob();
    let err = db.with_authenticated_session("bob", "pw",
        |s| s.execute("SELECT id FROM t.public.items WHERE id = 1")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_select_on_table_allows_bob_to_select_it() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    let rows = db.with_authenticated_session("bob", "pw",
        |s| s.execute("SELECT id FROM t.public.items WHERE id = 1")).unwrap();
    match rows {
        SqlResultSet::Rows { rows, .. } => {
            assert_eq!(rows, vec![vec![Value::Int64(1)]]);
        }
        other => panic!("expected Rows; got {other:?}"),
    }
}

#[test]
fn grant_on_one_table_does_not_allow_another() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw",
        |s| s.execute("SELECT id FROM t.public.other")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn revoke_select_takes_effect_after_session_reopen() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    db.with_authenticated_session("bob", "pw",
        |s| s.execute("SELECT id FROM t.public.items WHERE id = 1")).unwrap();
    db.execute("REVOKE SELECT ON t.public.items FROM bob").unwrap();
    // Grants are snapshotted at session start, so a NEW session reflects the revoke.
    let err = db.with_authenticated_session("bob", "pw",
        |s| s.execute("SELECT id FROM t.public.items WHERE id = 1")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn grant_select_rejects_unknown_user() {
    let (_dir, db) = admin_plus_bob();
    let err = db.execute("GRANT SELECT ON t.public.items TO ghost").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn grant_select_rejects_unknown_table() {
    let (_dir, db) = admin_plus_bob();
    let err = db.execute("GRANT SELECT ON t.public.ghost TO bob").unwrap_err();
    assert!(matches!(err, SqlError::Resolution(_)), "got {err:?}");
}

#[test]
fn grant_select_persists_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE DATABASE t").unwrap();
        db.execute("CREATE SCHEMA t.public").unwrap();
        db.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, PRIMARY KEY(id))")
            .unwrap();
        db.execute("INSERT INTO t.public.items(id) VALUES(1)").unwrap();
        db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    db.with_authenticated_session("bob", "pw",
        |s| s.execute("SELECT id FROM t.public.items WHERE id = 1")).unwrap();
}

#[test]
fn admin_with_global_read_bypasses_per_table_grants() {
    let (_dir, db) = admin_plus_bob();
    // alice is the admin; she must still select even without
    // any per-table grant because she has global READ.
    db.with_authenticated_session("alice", "pw",
        |s| s.execute("SELECT id FROM t.public.other")).unwrap();
}

#[test]
fn grant_select_cannot_be_used_for_insert() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    let err = db.with_authenticated_session("bob", "pw",
        |s| s.execute("INSERT INTO t.public.items(id) VALUES(99)")).unwrap_err();
    assert!(matches!(err, SqlError::Authz(_)), "got {err:?}");
}

#[test]
fn revoke_select_is_idempotent() {
    let (_dir, db) = admin_plus_bob();
    db.execute("REVOKE SELECT ON t.public.items FROM bob").unwrap();
    db.execute("REVOKE SELECT ON t.public.items FROM bob").unwrap();
}

#[test]
fn grant_select_is_idempotent() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
}

#[test]
fn snapshot_shows_per_table_grants() {
    let (_dir, db) = admin_plus_bob();
    db.execute("GRANT SELECT ON t.public.items TO bob").unwrap();
    let snap = db.user_catalog_snapshot();
    let bob = snap.get("bob").unwrap();
    assert_eq!(bob.table_grants().len(), 1,
        "bob should have one table grant, got {:?}", bob.table_grants());
}
