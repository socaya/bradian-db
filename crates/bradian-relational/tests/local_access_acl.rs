//! ADR 0321 — POSIX ACL inspection in local_access.

use bradian_relational::local_access::{
    check_posix_acl, ensure_private_root_with_acl, AclPolicy, PrivateRootPolicy,
};
use tempfile::tempdir;

#[cfg(unix)]
fn mk_private(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let root = dir.join("root");
    std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    root
}

#[test]
#[cfg(unix)]
fn ignore_policy_is_noop() {
    let scratch = tempdir().unwrap();
    let root = mk_private(scratch.path());
    ensure_private_root_with_acl(&root,
        PrivateRootPolicy::RequireExisting, AclPolicy::Ignore).unwrap();
}

#[test]
#[cfg(unix)]
fn strict_reject_without_acl_passes() {
    // A freshly-created private dir has no extended ACL on
    // macOS or on tmpfs — both common test environments.
    let scratch = tempdir().unwrap();
    let root = mk_private(scratch.path());
    // Strict should also pass when no ACL is present.
    ensure_private_root_with_acl(&root,
        PrivateRootPolicy::RequireExisting, AclPolicy::StrictReject).unwrap();
}

#[test]
#[cfg(unix)]
fn check_posix_acl_ignore_always_ok() {
    let scratch = tempdir().unwrap();
    let root = mk_private(scratch.path());
    check_posix_acl(&root, AclPolicy::Ignore).unwrap();
}

#[test]
#[cfg(unix)]
fn check_posix_acl_strict_without_acl_ok() {
    let scratch = tempdir().unwrap();
    let root = mk_private(scratch.path());
    check_posix_acl(&root, AclPolicy::StrictReject).unwrap();
}
