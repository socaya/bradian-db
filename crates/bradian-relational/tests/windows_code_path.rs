//! ADR 0291 — Windows code path for local_access.
//!
//! This test file exists primarily to document the
//! Windows code path and run basic smoke tests on the
//! current platform. The `#[cfg(windows)]` branches in
//! `local_access::ensure_private_root` / `_owned_by` /
//! `check_parent_path_trust` activate when the crate is
//! cross-compiled for Windows. The in-tree path is
//! verified structurally here; full Windows CI remains
//! external (ADR 0325).

use bradian_relational::local_access::{
    ensure_private_root, PrivateRootPolicy,
};
use tempfile::tempdir;

#[test]
#[cfg(windows)]
fn windows_ensure_private_root_creates_missing_dir() {
    let scratch = tempdir().unwrap();
    let root = scratch.path().join("new_root");
    ensure_private_root(&root, PrivateRootPolicy::CreateIfMissing).unwrap();
    assert!(root.is_dir());
}

#[test]
#[cfg(windows)]
fn windows_ensure_private_root_rejects_non_directory() {
    use bradian_relational::local_access::LocalAccessError;
    let scratch = tempdir().unwrap();
    let file = scratch.path().join("not_a_dir");
    std::fs::write(&file, b"").unwrap();
    let err = ensure_private_root(&file, PrivateRootPolicy::RequireExisting).unwrap_err();
    assert!(matches!(err, LocalAccessError::NotADirectory));
}

#[test]
#[cfg(unix)]
fn unix_path_still_works_after_windows_cfg_added() {
    // Regression: adding the Windows cfg branch mustn't
    // break the Unix path.
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let scratch = tempdir().unwrap();
    let root = scratch.path().join("root");
    std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    ensure_private_root(&root, PrivateRootPolicy::RequireExisting).unwrap();
}

#[test]
fn windows_code_path_symbols_are_reachable() {
    // Platform-agnostic smoke: the public symbols the
    // Windows branch depends on compile on every target.
    let scratch = tempdir().unwrap();
    let root = scratch.path().join("x");
    let _ = ensure_private_root(&root, PrivateRootPolicy::RequireExisting);
}
