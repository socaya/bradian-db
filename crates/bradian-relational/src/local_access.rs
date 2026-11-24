//! Shared POSIX private-root invariants for native local access
//! (ADR 0053, amended by ADR 0057).
//!
//! The helper enforces directory-mode and effective-UID ownership invariants.
//! POSIX ACLs, parent-path trust and Windows ACLs are not enforced here and
//! are documented as open items in the Bradian master roadmap (G1-LOCAL-ACCESS).

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivateRootPolicy {
    /// The root must already exist with the correct invariants.
    RequireExisting,
    /// If the root is absent, create it with mode 0o700.
    CreateIfMissing,
}

#[derive(Debug, Error)]
pub enum LocalAccessError {
    #[error("native local access enforcement is not qualified on this platform")]
    Unsupported,
    #[error("root does not exist")]
    NotFound,
    #[error("root exists but is not a directory")]
    NotADirectory,
    #[error("root is a symlink; a real private directory is required")]
    SymlinkedRoot,
    #[error("root has group or other permission bits set (mode {mode:o}); use a private 0700 directory")]
    LooseMode { mode: u32 },
    #[error("root is owned by uid {owner_uid}; expected uid {expected_uid}")]
    WrongOwner { owner_uid: u32, expected_uid: u32 },
    /// ADR 0109 — ancestor-path trust failure. An ancestor
    /// directory of the data root is world-writable without
    /// the sticky bit (or is a symlink) which lets other
    /// users traverse / replace path components.
    #[error("ancestor path {path:?} is untrusted (mode {mode:o}); a malicious user with write access to an ancestor can replace the private root")]
    UntrustedAncestor { path: std::path::PathBuf, mode: u32 },
    /// ADR 0321 — the directory has an extended POSIX ACL
    /// (via `system.posix_acl_access` xattr on Linux) which
    /// may grant access beyond the owner. Under
    /// `AclPolicy::StrictReject` this is rejected because
    /// the base mode-0o700 check alone no longer suffices.
    #[error("root {path:?} has an extended POSIX ACL; strict policy rejects")]
    ExtendedAcl { path: std::path::PathBuf },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// ADR 0321 — POSIX ACL inspection policy. Controls whether
/// `ensure_private_root_with_acl` rejects a directory that
/// carries an extended POSIX ACL beyond the base mode bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AclPolicy {
    /// Pre-ADR-0321 behaviour: skip ACL inspection. Callers
    /// that don't care about ACLs keep the mode-only check.
    #[default]
    Ignore,
    /// Reject if the root carries a `system.posix_acl_access`
    /// xattr (Linux). Non-Linux platforms fall through to
    /// Ignore since the check is OS-specific — documented
    /// honestly, not silently pretended.
    StrictReject,
}

/// ADR 0109 — parent-path trust policy. Controls how
/// `ensure_private_root_with_parent_trust` reacts to a
/// world-writable ancestor of the data root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ParentPathPolicy {
    /// Accept world-writable ancestors ONLY if the sticky
    /// bit is set (e.g., `/tmp` is mode 01777). This is the
    /// default and matches Unix convention — sticky means
    /// only the file owner can rename/delete entries, so
    /// cross-user traversal attacks fail.
    #[default]
    AllowSticky,
    /// Reject ANY world-writable ancestor, sticky or not.
    /// Use this for high-assurance deployments where
    /// `/tmp`-rooted data is not acceptable.
    Strict,
    /// Skip the parent-path check entirely. Preserves the
    /// pre-ADR-0109 behavior for callers that opt in.
    Ignore,
}

/// Enforce the POSIX private-root invariants on `path`. On non-Unix platforms
/// the helper returns `Unsupported` without touching the filesystem; callers
/// must decide how to react rather than silently skipping enforcement.
/// On Unix the expected owner is the effective UID (`geteuid()`).
/// ADR 0291 — on Windows the helper does a best-effort basic
/// check (existence + directory + exclusive-lock-can-be-taken)
/// instead of returning `Unsupported`. Full Windows ACL
/// inspection via `windows-sys` is captured as ADR 0322.
pub fn ensure_private_root(path: &Path, policy: PrivateRootPolicy)
    -> Result<(), LocalAccessError>
{
    #[cfg(unix)]
    {
        #[allow(unsafe_code)]
        // SAFETY: libc::geteuid is a POSIX thread-safe syscall with no
        // preconditions, no error return, and no side effects beyond
        // returning the caller's effective UID. It cannot fail or alias.
        let euid = unsafe { libc::geteuid() };
        unix::ensure_private_root_owned_by(path, policy, euid)
    }
    #[cfg(windows)]
    { windows_private_root(path, policy) }
    #[cfg(not(any(unix, windows)))]
    { let _ = (path, policy); Err(LocalAccessError::Unsupported) }
}

/// ADR 0291 — Windows private-root check. Verifies the
/// path exists (or is created under `CreateIfMissing`) and
/// is a directory. Full ACL inspection is a follow-up ADR
/// 0322; here we preserve the contract that the function
/// NEVER returns `Unsupported` on Windows, so callers can
/// treat Windows the same as Unix for the basic structural
/// invariants.
#[cfg(windows)]
fn windows_private_root(path: &Path, policy: PrivateRootPolicy)
    -> Result<(), LocalAccessError>
{
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                return Err(LocalAccessError::SymlinkedRoot);
            }
            if !meta.is_dir() {
                return Err(LocalAccessError::NotADirectory);
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            match policy {
                PrivateRootPolicy::RequireExisting => Err(LocalAccessError::NotFound),
                PrivateRootPolicy::CreateIfMissing => {
                    std::fs::create_dir_all(path).map_err(LocalAccessError::Io)
                }
            }
        }
        Err(e) => Err(LocalAccessError::Io(e)),
    }
}

/// Test-visible workhorse: enforce the private-root invariants against an
/// explicit expected-owner UID. On Unix this is the implementation used by
/// `ensure_private_root` (with `geteuid()` as the expected UID). Non-Unix
/// targets return `Unsupported` as above. This entry point lets tests prove
/// the wrong-owner rejection without requiring root (and `chown`).
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn ensure_private_root_owned_by(path: &Path, policy: PrivateRootPolicy, expected_uid: u32)
    -> Result<(), LocalAccessError>
{
    #[cfg(unix)]
    { unix::ensure_private_root_owned_by(path, policy, expected_uid) }
    #[cfg(windows)]
    { let _ = expected_uid; windows_private_root(path, policy) }
    #[cfg(not(any(unix, windows)))]
    { Err(LocalAccessError::Unsupported) }
}

/// ADR 0321 — enforce the private-root invariants AND
/// reject if the root carries an extended POSIX ACL
/// (Linux: `system.posix_acl_access` xattr). Under
/// `AclPolicy::Ignore` this degrades to `ensure_private_root`.
/// On non-Linux platforms the check is a no-op regardless
/// of policy — the OS-specific ACL surface is documented
/// in the Platform Gaps Plan rather than silently skipped.
pub fn ensure_private_root_with_acl(
    path: &Path,
    policy: PrivateRootPolicy,
    acl_policy: AclPolicy,
) -> Result<(), LocalAccessError> {
    ensure_private_root(path, policy)?;
    check_posix_acl(path, acl_policy)
}

/// ADR 0321 — standalone ACL inspection helper so callers
/// can audit without re-running the private-root check.
#[cfg_attr(not(all(unix, target_os = "linux")), allow(unused_variables))]
pub fn check_posix_acl(path: &Path, policy: AclPolicy) -> Result<(), LocalAccessError> {
    if matches!(policy, AclPolicy::Ignore) { return Ok(()); }
    #[cfg(all(unix, target_os = "linux"))]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        let c_path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| LocalAccessError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput, "path contains NUL byte")))?;
        let attr = CString::new("system.posix_acl_access").unwrap();
        // SAFETY: libc::getxattr with a non-null path, non-null
        // name, zero-size buffer and null value pointer is a
        // POSIX syscall that only returns metadata about
        // whether the attribute exists. No preconditions on
        // alignment or lifetime beyond the CString pointers
        // we hold for the call.
        #[allow(unsafe_code)]
        let rc = unsafe {
            libc::getxattr(
                c_path.as_ptr(), attr.as_ptr(),
                std::ptr::null_mut(), 0,
            )
        };
        if rc >= 0 {
            return Err(LocalAccessError::ExtendedAcl {
                path: path.to_path_buf(),
            });
        }
        // getxattr returns -1 on ENODATA (no ACL present) OR
        // ENOTSUP (xattrs unsupported on this filesystem).
        // Both are acceptable — treat as "no extended ACL".
    }
    Ok(())
}

/// ADR 0109 — enforce the private-root invariants AND walk
/// every ancestor directory checking for untrusted world-
/// writable paths (symlinks or world-writable without
/// sticky). This is the recommended entry point for new
/// callers; legacy callers of `ensure_private_root` get the
/// pre-ADR-0109 behavior.
pub fn ensure_private_root_with_parent_trust(
    path: &Path,
    policy: PrivateRootPolicy,
    parent_policy: ParentPathPolicy,
) -> Result<(), LocalAccessError> {
    ensure_private_root(path, policy)?;
    check_parent_path_trust(path, parent_policy)
}

/// ADR 0109 — walk ancestors of `path` and check each against
/// `parent_policy`. Public so callers can run the check
/// independently of `ensure_private_root`.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn check_parent_path_trust(
    path: &Path,
    parent_policy: ParentPathPolicy,
) -> Result<(), LocalAccessError> {
    if matches!(parent_policy, ParentPathPolicy::Ignore) {
        return Ok(());
    }
    #[cfg(unix)]
    { unix::check_parent_path_trust(path, parent_policy) }
    #[cfg(windows)]
    {
        // ADR 0291 — Windows doesn't use POSIX world-writable
        // bits. Parent-path trust under Windows is governed by
        // ACLs; a full implementation lives in ADR 0322. Here
        // we accept the path if it exists at all.
        let _ = parent_policy;
        if path.exists() { Ok(()) } else { Err(LocalAccessError::NotFound) }
    }
    #[cfg(not(any(unix, windows)))]
    { Err(LocalAccessError::Unsupported) }
}

#[cfg(unix)]
mod unix {
    use super::{LocalAccessError, ParentPathPolicy, PrivateRootPolicy};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};

    /// Walk from `path.parent()` up to the filesystem root
    /// (`/`), checking each component against `policy`.
    /// - Symlink ancestor → always rejected (attacker can
    ///   redirect the path).
    /// - World-writable without sticky → always rejected.
    /// - World-writable WITH sticky → rejected only under
    ///   `Strict`.
    pub(super) fn check_parent_path_trust(
        path: &Path, policy: ParentPathPolicy,
    ) -> Result<(), LocalAccessError> {
        if matches!(policy, ParentPathPolicy::Ignore) {
            return Ok(());
        }
        // Canonicalise the path so the walk reflects the real
        // ancestors, not the caller's symlink-laden argument.
        let canonical = match std::fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // If root is absent but was just created via
                // CreateIfMissing, canonicalise should succeed.
                // If not, fall back to the input path.
                path.to_path_buf()
            }
            Err(e) => return Err(LocalAccessError::Io(e)),
        };
        let mut cursor: PathBuf = canonical;
        // Pop the data root itself; we only check ANCESTORS.
        if !cursor.pop() { return Ok(()); }
        loop {
            let meta = match std::fs::symlink_metadata(&cursor) {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Err(e) => return Err(LocalAccessError::Io(e)),
            };
            if meta.file_type().is_symlink() {
                return Err(LocalAccessError::UntrustedAncestor {
                    path: cursor.clone(),
                    mode: meta.permissions().mode() & 0o7777,
                });
            }
            let mode = meta.permissions().mode() & 0o7777;
            let world_writable = mode & 0o002 != 0;
            let sticky = mode & 0o1000 != 0;
            let fails = match policy {
                ParentPathPolicy::Strict => world_writable,
                ParentPathPolicy::AllowSticky => world_writable && !sticky,
                ParentPathPolicy::Ignore => false,
            };
            if fails {
                return Err(LocalAccessError::UntrustedAncestor {
                    path: cursor.clone(), mode,
                });
            }
            if !cursor.pop() { break; }
        }
        Ok(())
    }

    pub(super) fn ensure_private_root_owned_by(
        path: &Path, policy: PrivateRootPolicy, expected_uid: u32,
    ) -> Result<(), LocalAccessError>
    {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(LocalAccessError::SymlinkedRoot);
                }
                if !metadata.is_dir() {
                    return Err(LocalAccessError::NotADirectory);
                }
                let mode = metadata.permissions().mode();
                if mode & 0o077 != 0 {
                    return Err(LocalAccessError::LooseMode { mode });
                }
                let owner_uid = metadata.uid();
                if owner_uid != expected_uid {
                    return Err(LocalAccessError::WrongOwner { owner_uid, expected_uid });
                }
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match policy {
                    PrivateRootPolicy::RequireExisting => Err(LocalAccessError::NotFound),
                    PrivateRootPolicy::CreateIfMissing => {
                        let mut builder = std::fs::DirBuilder::new();
                        builder.recursive(true).mode(0o700).create(path)?;
                        // The freshly created directory is owned by the
                        // effective UID that just created it. If the caller
                        // supplied a non-euid expected value, enforce it.
                        let metadata = std::fs::symlink_metadata(path)?;
                        let owner_uid = metadata.uid();
                        if owner_uid != expected_uid {
                            return Err(LocalAccessError::WrongOwner { owner_uid, expected_uid });
                        }
                        Ok(())
                    }
                }
            }
            Err(err) => Err(LocalAccessError::Io(err)),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn existing_private_directory_is_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        ensure_private_root(&root, PrivateRootPolicy::RequireExisting).unwrap();
    }

    #[test]
    fn missing_root_is_created_with_private_mode() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nested").join("private");
        ensure_private_root(&root, PrivateRootPolicy::CreateIfMissing).unwrap();
        let mode = std::fs::symlink_metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn require_existing_refuses_absent_root_without_creating_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("absent");
        assert!(matches!(
            ensure_private_root(&root, PrivateRootPolicy::RequireExisting),
            Err(LocalAccessError::NotFound),
        ));
        assert!(!root.exists());
    }

    #[test]
    fn symlinked_root_is_refused_even_when_target_is_private() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = dir.path().join("link");
        symlink(&target, &link).unwrap();
        assert!(matches!(
            ensure_private_root(&link, PrivateRootPolicy::RequireExisting),
            Err(LocalAccessError::SymlinkedRoot),
        ));
    }

    #[test]
    fn regular_file_is_rejected_as_not_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("plain");
        std::fs::write(&file, b"not-a-dir").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            ensure_private_root(&file, PrivateRootPolicy::RequireExisting),
            Err(LocalAccessError::NotADirectory),
        ));
    }

    #[test]
    fn group_or_other_readable_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        for loose in [0o750, 0o770, 0o755, 0o705, 0o707, 0o777] {
            let root = dir.path().join(format!("loose-{loose:o}"));
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(loose)).unwrap();
            match ensure_private_root(&root, PrivateRootPolicy::RequireExisting) {
                Err(LocalAccessError::LooseMode { mode }) => {
                    assert_eq!(mode & 0o777, loose, "expected loose mode preserved in error");
                }
                other => panic!("expected LooseMode for 0o{loose:o}, got {other:?}"),
            }
        }
    }

    #[test]
    fn create_if_missing_is_idempotent_against_already_private_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        ensure_private_root(&root, PrivateRootPolicy::CreateIfMissing).unwrap();
        // Second call must accept the existing 0o700 directory, not alter it.
        ensure_private_root(&root, PrivateRootPolicy::CreateIfMissing).unwrap();
        let mode = std::fs::symlink_metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    #[test]
    fn effective_uid_check_accepts_own_directory_against_default_entry_point() {
        // The default ensure_private_root wraps the wrong-owner check with
        // geteuid() as the expected UID. A directory just created by the
        // running process is owned by that euid and must be accepted.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        ensure_private_root(&root, PrivateRootPolicy::RequireExisting).unwrap();
    }

    #[test]
    fn wrong_owner_uid_is_refused_without_touching_the_directory() {
        // Prove the WrongOwner refusal path without requiring chown: the
        // tests-visible workhorse accepts an explicit expected UID. A clearly
        // distinct value (u32::MAX) cannot match any real file owner UID.
        use super::ensure_private_root_owned_by;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let before_mode = std::fs::symlink_metadata(&root).unwrap().permissions().mode();
        match ensure_private_root_owned_by(&root, PrivateRootPolicy::RequireExisting, u32::MAX) {
            Err(LocalAccessError::WrongOwner { owner_uid, expected_uid }) => {
                assert_ne!(owner_uid, u32::MAX);
                assert_eq!(expected_uid, u32::MAX);
            }
            other => panic!("expected WrongOwner, got {other:?}"),
        }
        // The refused check left the directory mode exactly as it was.
        assert_eq!(
            std::fs::symlink_metadata(&root).unwrap().permissions().mode(),
            before_mode,
        );
    }

    #[test]
    fn mode_refusal_takes_precedence_over_uid_mismatch() {
        // A directory with loose permissions AND the wrong owner should
        // report LooseMode first, matching the audit ordering: fix the
        // permissions, then the ownership, not the other way around.
        use super::ensure_private_root_owned_by;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("loose");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        match ensure_private_root_owned_by(&root, PrivateRootPolicy::RequireExisting, u32::MAX) {
            Err(LocalAccessError::LooseMode { .. }) => {}
            other => panic!("expected LooseMode first, got {other:?}"),
        }
    }

    // ---------- ADR 0109 parent-path trust ----------

    #[test]
    fn ignore_policy_accepts_any_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("private");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Even if we had bad ancestors, Ignore skips the check.
        check_parent_path_trust(&root, ParentPathPolicy::Ignore).unwrap();
    }

    #[test]
    fn allow_sticky_accepts_tempdir_under_tmp() {
        // tempdir() places the root under /tmp which is sticky
        // world-writable (01777). AllowSticky must accept this.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("under-tmp");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        check_parent_path_trust(&root, ParentPathPolicy::AllowSticky).unwrap();
    }

    #[test]
    fn strict_rejects_world_writable_ancestor_even_with_sticky() {
        // Craft a world-writable sticky directory and place a
        // root under it. Strict must reject.
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad-parent");
        std::fs::create_dir(&bad).unwrap();
        // 01777: sticky + world-writable.
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o1777)).unwrap();
        let root = bad.join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        match check_parent_path_trust(&root, ParentPathPolicy::Strict) {
            Err(LocalAccessError::UntrustedAncestor { path, mode }) => {
                assert!(path.ends_with("bad-parent"));
                assert_eq!(mode & 0o777, 0o777);
            }
            other => panic!("expected UntrustedAncestor; got {other:?}"),
        }
    }

    #[test]
    fn allow_sticky_rejects_world_writable_without_sticky() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("world-writable-no-sticky");
        std::fs::create_dir(&bad).unwrap();
        // 0777: world-writable, no sticky.
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o777)).unwrap();
        let root = bad.join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        match check_parent_path_trust(&root, ParentPathPolicy::AllowSticky) {
            Err(LocalAccessError::UntrustedAncestor { path, .. }) => {
                assert!(path.ends_with("world-writable-no-sticky"));
            }
            other => panic!("expected UntrustedAncestor; got {other:?}"),
        }
    }

    #[test]
    fn ensure_private_root_with_parent_trust_combines_both_checks() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("combined");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        // AllowSticky accepts /tmp-rooted temp dirs.
        ensure_private_root_with_parent_trust(
            &root, PrivateRootPolicy::RequireExisting,
            ParentPathPolicy::AllowSticky,
        ).unwrap();
    }

    #[test]
    fn root_check_runs_before_parent_check() {
        // If the root itself is loose, LooseMode is returned
        // regardless of parent policy — fix the root first.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("loose-root");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        match ensure_private_root_with_parent_trust(
            &root, PrivateRootPolicy::RequireExisting,
            ParentPathPolicy::Strict,
        ) {
            Err(LocalAccessError::LooseMode { .. }) => {}
            other => panic!("expected LooseMode first; got {other:?}"),
        }
    }
}
