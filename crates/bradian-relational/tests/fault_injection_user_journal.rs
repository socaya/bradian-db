//! ADR 0253 — fault injection on the PersistentUserCatalog
//! journal. Pins the same middle-flip / tail-truncate /
//! zeroed-magic recovery claims as the RowStore WAL tests.

use bradian_relational::fault_injection::{
    file_len, flip_byte_at, truncate_file_to, zero_range,
};
use bradian_relational::persistent_user_catalog::{
    PersistentUserCatalog, PersistentUserCatalogError,
};
use tempfile::tempdir;

/// Seed a user catalog with three users and return the path
/// to its WAL after close.
fn seeded_user_wal(dir: &std::path::Path) -> std::path::PathBuf {
    let cat = PersistentUserCatalog::open(dir).unwrap();
    cat.create_user("alice", "pw1").unwrap();
    cat.create_user("bob", "pw2").unwrap();
    cat.create_user("carol", "pw3").unwrap();
    drop(cat);
    dir.join("rows.wal")
}

#[test]
fn clean_reopen_matches_committed_users() {
    let dir = tempdir().unwrap();
    let _wal = seeded_user_wal(dir.path());
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    cat.verify_password("alice", "pw1").unwrap();
    cat.verify_password("bob", "pw2").unwrap();
    cat.verify_password("carol", "pw3").unwrap();
}

#[test]
fn flipping_a_middle_user_wal_byte_is_detected() {
    let dir = tempdir().unwrap();
    let wal = seeded_user_wal(dir.path());
    let len = file_len(&wal).unwrap();
    // Middle of the file — lands inside a committed frame body
    // (user record JSON or its envelope).
    flip_byte_at(&wal, len / 2).unwrap();
    let err = PersistentUserCatalog::open(dir.path())
        .expect_err("flipped byte must not open cleanly");
    // Either the underlying RowStore surfaces Storage(Io/Corruption)
    // or the journal surfaces Corruption during replay — both
    // are explicit rejections.
    assert!(
        matches!(
            err,
            PersistentUserCatalogError::Storage(_) | PersistentUserCatalogError::Corruption(_)
                | PersistentUserCatalogError::Encoding(_),
        ),
        "expected Storage/Corruption/Encoding, got {err:?}",
    );
}

#[test]
fn truncating_tail_either_drops_or_rejects() {
    let dir = tempdir().unwrap();
    let wal = seeded_user_wal(dir.path());
    let len = file_len(&wal).unwrap();
    truncate_file_to(&wal, len - 1).unwrap();
    match PersistentUserCatalog::open(dir.path()) {
        Ok(cat) => {
            // If recovery dropped the torn frame, every user
            // that was committed BEFORE it must still verify.
            // At minimum alice (the first CreateUser) must be
            // intact — she's much earlier than the tail byte.
            cat.verify_password("alice", "pw1").unwrap();
        }
        Err(err) => assert!(
            matches!(
                err,
                PersistentUserCatalogError::Storage(_) | PersistentUserCatalogError::Corruption(_)
                    | PersistentUserCatalogError::Encoding(_),
            ),
            "expected Storage/Corruption/Encoding, got {err:?}",
        ),
    }
}

#[test]
fn zeroing_wal_magic_is_rejected() {
    let dir = tempdir().unwrap();
    let wal = seeded_user_wal(dir.path());
    zero_range(&wal, 0, 4).unwrap();
    let err = PersistentUserCatalog::open(dir.path())
        .expect_err("zeroed magic must not open cleanly");
    assert!(
        matches!(
            err,
            PersistentUserCatalogError::Storage(_) | PersistentUserCatalogError::Corruption(_),
        ),
        "expected Storage/Corruption, got {err:?}",
    );
}

#[test]
fn deleting_wal_resets_catalog_to_empty() {
    // Wiping the WAL entirely is not a fault scenario per se —
    // it's deliberate reset. Reopen must succeed with an empty
    // catalog so the operator can recreate users.
    let dir = tempdir().unwrap();
    let wal = seeded_user_wal(dir.path());
    std::fs::remove_file(&wal).unwrap();
    let cat = PersistentUserCatalog::open(dir.path()).unwrap();
    assert!(!cat.contains("alice"));
    assert_eq!(cat.last_lsn(), 1, "expected only the version-marker frame");
}
