//! Shared POSIX private-root invariants for native local access (ADR 0053).
//!
//! The helper enforces directory-mode invariants ONLY. Effective-UID ownership,
//! POSIX ACLs, parent-path trust and Windows ACLs are not enforced here and are
//! documented as open items in the Bradian master roadmap (G1-LOCAL-ACCESS).

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
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Enforce the POSIX private-root invariants on `path`. On non-Unix platforms
/// the helper returns `Unsupported` without touching the filesystem; callers
/// must decide how to react rather than silently skipping enforcement.
pub fn ensure_private_root(path: &Path, policy: PrivateRootPolicy)
    -> Result<(), LocalAccessError>
{
    #[cfg(unix)]
    { unix::ensure_private_root(path, policy) }
    #[cfg(not(unix))]
    { let _ = (path, policy); Err(LocalAccessError::Unsupported) }
}

#[cfg(unix)]
mod unix {
    use super::{LocalAccessError, PrivateRootPolicy};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    use std::path::Path;

    pub(super) fn ensure_private_root(path: &Path, policy: PrivateRootPolicy)
        -> Result<(), LocalAccessError>
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
                Ok(())
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match policy {
                    PrivateRootPolicy::RequireExisting => Err(LocalAccessError::NotFound),
                    PrivateRootPolicy::CreateIfMissing => {
                        let mut builder = std::fs::DirBuilder::new();
                        builder.recursive(true).mode(0o700).create(path)?;
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
}
