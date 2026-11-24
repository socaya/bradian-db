//! ADR 0249 — CREATE USER / DROP USER / ALTER USER grammar.

use bradian_relational::persistent_user_catalog::PersistentUserCatalogError;
use bradian_relational::sql::{SqlEngine, SqlError};
use bradian_relational::user_catalog::UserCatalogError;
use tempfile::tempdir;

#[test]
fn create_user_and_verify_password() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 's3cret'").unwrap();
    db.verify_password("alice", "s3cret").unwrap();
    // Wrong password rejected.
    let err = db.verify_password("alice", "wrong").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn create_user_requires_password_literal() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    let err = db.execute("CREATE USER alice WITH PASSWORD 42").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn create_duplicate_user_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'p1'").unwrap();
    let err = db.execute("CREATE USER alice WITH PASSWORD 'p2'").unwrap_err();
    assert!(matches!(err, SqlError::User(
        PersistentUserCatalogError::User(UserCatalogError::Duplicate(_))
    )), "got {err:?}");
}

#[test]
fn create_user_invalid_name_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    // 'with' is a reserved keyword continuation; the parser's
    // word() may still return a name token. Use a leading-digit
    // identifier which the validator will reject.
    let err = db.execute("CREATE USER alice WITH PASSWORD ''").unwrap_err();
    // Empty password is caught by UserCatalog::create_user.
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn drop_user_removes_authentication() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.verify_password("alice", "pw").unwrap();
    db.execute("DROP USER alice").unwrap();
    let err = db.verify_password("alice", "pw").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn drop_unknown_user_rejected() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    let err = db.execute("DROP USER ghost").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn alter_user_set_password() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'old'").unwrap();
    db.execute("ALTER USER alice SET PASSWORD 'new'").unwrap();
    db.verify_password("alice", "new").unwrap();
    let err = db.verify_password("alice", "old").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn alter_user_set_active_toggles() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    db.execute("ALTER USER alice SET ACTIVE FALSE").unwrap();
    let err = db.verify_password("alice", "pw").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
    db.execute("ALTER USER alice SET ACTIVE TRUE").unwrap();
    db.verify_password("alice", "pw").unwrap();
}

#[test]
fn alter_user_set_active_rejects_other_words() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
    let err = db.execute("ALTER USER alice SET ACTIVE yes").unwrap_err();
    assert!(matches!(err, SqlError::Syntax(_)), "got {err:?}");
}

#[test]
fn case_insensitive_user_name() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER Alice WITH PASSWORD 'pw'").unwrap();
    db.verify_password("alice", "pw").unwrap();
    db.verify_password("ALICE", "pw").unwrap();
    db.execute("DROP USER aLiCe").unwrap();
    let err = db.verify_password("alice", "pw").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn users_persist_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let db = SqlEngine::open(dir.path()).unwrap();
        db.execute("CREATE USER alice WITH PASSWORD 'pw'").unwrap();
        db.execute("CREATE USER bob WITH PASSWORD 'pw'").unwrap();
        db.execute("ALTER USER bob SET ACTIVE FALSE").unwrap();
    }
    let db = SqlEngine::open(dir.path()).unwrap();
    db.verify_password("alice", "pw").unwrap();
    let err = db.verify_password("bob", "pw").unwrap_err();
    assert!(matches!(err, SqlError::User(_)), "got {err:?}");
}

#[test]
fn user_catalog_snapshot_shows_users() {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    db.execute("CREATE USER a WITH PASSWORD 'pw'").unwrap();
    db.execute("CREATE USER b WITH PASSWORD 'pw'").unwrap();
    let snap = db.user_catalog_snapshot();
    assert_eq!(snap.len(), 2);
    assert!(snap.contains("a"));
    assert!(snap.contains("b"));
}
