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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Enforce the POSIX private-root invariants on `path`. On non-Unix platforms
/// the helper returns `Unsupported` without touching the filesystem; callers
/// must decide how to react rather than silently skipping enforcement.
/// On Unix the expected owner is the effective UID (`geteuid()`).
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
    #[cfg(not(unix))]
    { let _ = (path, policy); Err(LocalAccessError::Unsupported) }
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
    #[cfg(not(unix))]
    { Err(LocalAccessError::Unsupported) }
}

#[cfg(unix)]
mod unix {
    use super::{LocalAccessError, PrivateRootPolicy};
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    use std::path::Path;

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
}
