//! ADR 0250 — SqlEngine::with_authenticated_session.

use bradian_core::Capability;
use bradian_relational::sql::{SqlEngine, SqlError};
use tempfile::tempdir;

fn open_with_user() -> (tempfile::TempDir, SqlEngine) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    (dir, db)
}

#[test]
fn verify_then_execute_through_session() {
    let (_dir, db) = open_with_user();
    let principal = db.with_authenticated_session("alice", "pw", |s| {
        // The principal binding yields canonical lowercase name.
        assert_eq!(s.principal(), "alice");
        // Session can execute SELECT-style DDL under admin caps.
        s.execute("CREATE DATABASE t").unwrap();
        s.execute("CREATE SCHEMA t.public").unwrap();
        Ok(s.principal().to_string())
    }).unwrap();
    assert_eq!(principal, "alice");
    // Mutation persisted.
    let err = db.execute("CREATE DATABASE t").unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "duplicate create, got {err:?}");
}

#[test]
fn wrong_password_rejects_session() {
    let (_dir, db) = open_with_user();
    let err = db.with_authenticated_session("alice", "wrong", |_| Ok(()))
        .unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn unknown_user_rejects_session() {
    let (_dir, db) = open_with_user();
    let err = db.with_authenticated_session("ghost", "anything", |_| Ok(()))
        .unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn inactive_user_rejects_session() {
    let (_dir, db) = open_with_user();
    db.execute("ALTER USER alice SET ACTIVE FALSE").unwrap();
    let err = db.with_authenticated_session("alice", "pw", |_| Ok(()))
        .unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn principal_is_canonical_lowercase() {
    let (_dir, db) = open_with_user();
    db.with_authenticated_session("ALICE", "pw", |s| {
        assert_eq!(s.principal(), "alice");
        Ok(())
    }).unwrap();
}

#[test]
fn authenticated_session_has_admin_capabilities_for_now() {
    // ADR 0250 — placeholder until GRANT/roles land. Every
    // authenticated session gets Read / Write / Ddl / Admin.
    let (_dir, db) = open_with_user();
    db.with_authenticated_session("alice", "pw", |s| {
        let caps = s.capabilities();
        assert!(caps.contains(&Capability::Read));
        assert!(caps.contains(&Capability::Write));
        assert!(caps.contains(&Capability::Ddl));
        assert!(caps.contains(&Capability::Admin));
        Ok(())
    }).unwrap();
}

#[test]
fn session_dml_round_trips() {
    let (_dir, db) = open_with_user();
    db.with_authenticated_session("alice", "pw", |s| {
        s.execute("CREATE DATABASE t")?;
        s.execute("CREATE SCHEMA t.public")?;
        s.execute("CREATE TABLE t.public.items (id BIGINT NOT NULL, note TEXT, PRIMARY KEY(id))")?;
        s.execute("INSERT INTO t.public.items(id, note) VALUES(1, 'alpha')")?;
        let rs = s.execute("SELECT id FROM t.public.items WHERE id = 1")?;
        match rs {
            bradian_relational::sql::SqlResultSet::Rows { rows, .. } => {
                assert_eq!(rows.len(), 1);
            }
            other => panic!("expected Rows, got {other:?}"),
        }
        Ok(())
    }).unwrap();
}

#[test]
fn session_error_propagates() {
    let (_dir, db) = open_with_user();
    let err = db.with_authenticated_session("alice", "pw", |s| {
        s.execute("CREATE DATABASE t")?;
        // Duplicate — should surface as Catalog error.
        s.execute("CREATE DATABASE t")?;
        Ok(())
    }).unwrap_err();
    assert!(matches!(err, SqlError::Catalog(_)), "got {err:?}");
}

#[test]
fn two_sessions_distinct_principals() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
    db.with_authenticated_session("alice", "pw", |s| {
        assert_eq!(s.principal(), "alice");
        Ok(())
    }).unwrap();
    db.with_authenticated_session("bob", "pw", |s| {
        assert_eq!(s.principal(), "bob");
        Ok(())
    }).unwrap();
}
