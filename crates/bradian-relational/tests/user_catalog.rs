//! ADR 0247 — UserCatalog + argon2id.

use bradian_relational::user_catalog::{UserCatalog, UserCatalogError};

#[test]
fn create_and_verify_password() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "s3cret").unwrap();
    cat.verify_password("alice", "s3cret").unwrap();
}

#[test]
fn verify_wrong_password_rejects() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "s3cret").unwrap();
    assert_eq!(cat.verify_password("alice", "wrong"), Err(UserCatalogError::InvalidPassword));
}

#[test]
fn unknown_user_same_error_shape_as_wrong_password() {
    // Timing-safe + surface-safe: an attacker enumerating
    // usernames must not learn existence from the error.
    let cat = UserCatalog::new();
    assert_eq!(cat.verify_password("ghost", "anything"), Err(UserCatalogError::InvalidPassword));
}

#[test]
fn case_insensitive_username() {
    let mut cat = UserCatalog::new();
    cat.create_user("Alice", "s3cret").unwrap();
    cat.verify_password("ALICE", "s3cret").unwrap();
    cat.verify_password("alice", "s3cret").unwrap();
}

#[test]
fn duplicate_user_rejected() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "p1").unwrap();
    match cat.create_user("alice", "p2") {
        Err(UserCatalogError::Duplicate(n)) => assert_eq!(n, "alice"),
        other => panic!("expected Duplicate, got {other:?}"),
    }
}

#[test]
fn invalid_name_rejected() {
    let mut cat = UserCatalog::new();
    for bad in ["", "1starts_with_digit", "has space", "tooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooooolong", "has-dash"] {
        assert!(matches!(cat.create_user(bad, "pw"), Err(UserCatalogError::InvalidName(_))),
            "name {bad:?} should be rejected");
    }
}

#[test]
fn empty_password_rejected_on_create_and_set() {
    let mut cat = UserCatalog::new();
    assert_eq!(cat.create_user("alice", ""), Err(UserCatalogError::InvalidPassword));
    cat.create_user("alice", "ok").unwrap();
    assert_eq!(cat.set_password("alice", ""), Err(UserCatalogError::InvalidPassword));
}

#[test]
fn drop_user_removes() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "pw").unwrap();
    cat.drop_user("alice").unwrap();
    assert!(!cat.contains("alice"));
    assert_eq!(cat.verify_password("alice", "pw"), Err(UserCatalogError::InvalidPassword));
}

#[test]
fn drop_unknown_rejects() {
    let mut cat = UserCatalog::new();
    match cat.drop_user("ghost") {
        Err(UserCatalogError::NotFound(_)) => {}
        other => panic!("expected NotFound, got {other:?}"),
    }
}

#[test]
fn set_password_rotation() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "old").unwrap();
    cat.set_password("alice", "new").unwrap();
    assert_eq!(cat.verify_password("alice", "old"), Err(UserCatalogError::InvalidPassword));
    cat.verify_password("alice", "new").unwrap();
}

#[test]
fn inactive_user_cannot_authenticate() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "pw").unwrap();
    cat.set_active("alice", false).unwrap();
    assert_eq!(cat.verify_password("alice", "pw"), Err(UserCatalogError::InvalidPassword));
    cat.set_active("alice", true).unwrap();
    cat.verify_password("alice", "pw").unwrap();
}

#[test]
fn identical_passwords_produce_different_hashes() {
    // Confirms the salt is actually random.
    let mut cat = UserCatalog::new();
    cat.create_user("a", "same").unwrap();
    cat.create_user("b", "same").unwrap();
    let a = cat.get("a").unwrap().password_hash().to_owned();
    let b = cat.get("b").unwrap().password_hash().to_owned();
    assert_ne!(a, b, "identical passwords must not share a hash");
}

#[test]
fn stored_hash_round_trips_via_serde() {
    // Catch regressions in future persistence slices: the
    // serialized form must survive a round trip.
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "pw").unwrap();
    let json = serde_json::to_string(&cat).unwrap();
    let back: UserCatalog = serde_json::from_str(&json).unwrap();
    back.verify_password("alice", "pw").unwrap();
    assert_eq!(back.verify_password("alice", "wrong"), Err(UserCatalogError::InvalidPassword));
}

#[test]
fn users_iter_is_sorted() {
    let mut cat = UserCatalog::new();
    for n in ["zoe", "bob", "alice"] {
        cat.create_user(n, "pw").unwrap();
    }
    let names: Vec<_> = cat.users().map(|u| u.name.as_str()).collect();
    assert_eq!(names, vec!["alice", "bob", "zoe"]);
}

#[test]
fn stored_hash_is_argon2id_phc() {
    let mut cat = UserCatalog::new();
    cat.create_user("alice", "pw").unwrap();
    let phc = cat.get("alice").unwrap().password_hash();
    assert!(phc.starts_with("$argon2id$"), "expected argon2id PHC, got {phc}");
}
