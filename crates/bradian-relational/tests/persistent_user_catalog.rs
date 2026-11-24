//! ADR 0248 — PersistentUserCatalog.

use bradian_relational::persistent_user_catalog::{
    PersistentUserCatalog, PersistentUserCatalogError,
};
use bradian_relational::user_catalog::UserCatalogError;
use tempfile::tempdir;

#[test]
fn create_persist_reopen_verify() {
    let dir = tempdir().unwrap();
    {
        let cat = PersistentUserCatalog::open(dir.path()).unwrap();
        cat.create_user("alice", "s3cret").unwrap();
    }
    // Reopen — the user must persist and the password still verifies.
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    cat.verify_password("alice", "s3cret").unwrap();
    assert!(cat.contains("alice"));
}

#[test]
fn verify_wrong_password_rejects_after_reopen() {
    let dir = tempdir().unwrap();
    {
        let cat = PersistentUserCatalog::open(dir.path()).unwrap();
        cat.create_user("alice", "s3cret").unwrap();
    }
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    let err = cat.verify_password("alice", "wrong").unwrap_err();
    matches!(err, PersistentUserCatalogError::User(UserCatalogError::InvalidPassword));
}

#[test]
fn drop_persists_across_reopen() {
    let dir = tempdir().unwrap();
    {
        let cat = PersistentUserCatalog::open(dir.path()).unwrap();
        cat.create_user("alice", "pw").unwrap();
        cat.drop_user("alice").unwrap();
    }
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    assert!(!cat.contains("alice"));
}

#[test]
fn password_rotation_persists() {
    let dir = tempdir().unwrap();
    {
        let cat = PersistentUserCatalog::open(dir.path()).unwrap();
        cat.create_user("alice", "old").unwrap();
        cat.set_password("alice", "new").unwrap();
    }
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    cat.verify_password("alice", "new").unwrap();
    let err = cat.verify_password("alice", "old").unwrap_err();
    matches!(err, PersistentUserCatalogError::User(UserCatalogError::InvalidPassword));
}

#[test]
fn active_flag_persists() {
    let dir = tempdir().unwrap();
    {
        let cat = PersistentUserCatalog::open(dir.path()).unwrap();
        cat.create_user("alice", "pw").unwrap();
        cat.set_active("alice", false).unwrap();
    }
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    // Inactive user cannot authenticate.
    let err = cat.verify_password("alice", "pw").unwrap_err();
    matches!(err, PersistentUserCatalogError::User(UserCatalogError::InvalidPassword));
    cat.set_active("alice", true).unwrap();
    cat.verify_password("alice", "pw").unwrap();
}

#[test]
fn duplicate_create_rejects_without_journaling() {
    let dir = tempdir().unwrap();
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    cat.create_user("alice", "pw").unwrap();
    let before = cat.last_lsn();
    let err = cat.create_user("alice", "other").unwrap_err();
    matches!(err, PersistentUserCatalogError::User(UserCatalogError::Duplicate(_)));
    // Failed create must not advance the journal.
    assert_eq!(cat.last_lsn(), before);
}

#[test]
fn drop_unknown_rejects_without_journaling() {
    let dir = tempdir().unwrap();
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    let before = cat.last_lsn();
    let err = cat.drop_user("ghost").unwrap_err();
    matches!(err, PersistentUserCatalogError::User(UserCatalogError::NotFound(_)));
    assert_eq!(cat.last_lsn(), before);
}

#[test]
fn snapshot_reflects_current_state() {
    let dir = tempdir().unwrap();
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    cat.create_user("alice", "pw").unwrap();
    cat.create_user("bob", "pw").unwrap();
    cat.drop_user("alice").unwrap();
    let snap = cat.snapshot();
    assert!(!snap.contains("alice"));
    assert!(snap.contains("bob"));
}

#[test]
fn many_users_persist() {
    let dir = tempdir().unwrap();
    {
        let cat = PersistentUserCatalog::open(dir.path()).unwrap();
        for i in 0..12 {
            cat.create_user(&format!("user{i}"), &format!("pw{i}")).unwrap();
        }
    }
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    for i in 0..12 {
        cat.verify_password(&format!("user{i}"), &format!("pw{i}")).unwrap();
    }
}

#[test]
fn journal_lsn_advances_monotonically() {
    let dir = tempdir().unwrap();
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    let a = cat.last_lsn();
    cat.create_user("a", "pw").unwrap();
    let b = cat.last_lsn();
    cat.create_user("b", "pw").unwrap();
    let c = cat.last_lsn();
    cat.set_active("a", false).unwrap();
    let d = cat.last_lsn();
    assert!(a < b);
    assert!(b < c);
    assert!(c < d);
}
