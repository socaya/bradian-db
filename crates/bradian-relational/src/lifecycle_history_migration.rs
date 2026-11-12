//! G25 verified G18 lifecycle-managed source -> G23 historical database cutover.
//!
//! G19 first captures and independently verifies the authoritative catalog,
//! shared WAL domain, and lifecycle journal. Only a recovered temporary COPY
//! loses its legacy lifecycle component before the existing G23 importer
//! converts it. The original database remains unchanged apart from any
//! coordinator's normal pending-intent reconciliation.
//! Historical versions before cutover cannot be reconstructed from G18.

use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::database_backup::{restore_database, DatabaseBackupError};
use crate::history_migration::{migrate_shared_to_history, HistoryMigrationError, HistoryMigrationReport};
use crate::lifecycle::{CatalogLifecycle, LifecycleError};

#[derive(Debug, Error)]
pub enum LifecycleHistoryMigrationError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Lifecycle(#[from] LifecycleError),
    #[error(transparent)]
    Backup(#[from] DatabaseBackupError),
    #[error(transparent)]
    Migration(#[from] HistoryMigrationError),
    #[error("G25 lifecycle history migration refused: {0}")]
    Invalid(String),
}

pub type LifecycleHistoryMigrationResult<T> = Result<T, LifecycleHistoryMigrationError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleHistoryMigrationReport {
    pub imported: HistoryMigrationReport,
    pub source_lifecycle_lsn: u64,
    pub captured_catalog_lsn: u64,
    pub captured_domain_lsn: u64,
}

/// Prepare three destination-parent sibling paths without writing anything.
/// Failed or interrupted migrations deliberately preserve their private
/// staging directories for operator inspection; existing staging refuses a
/// retry rather than silently deleting potentially valuable recovered data.
fn paths(
    source: &Path, requested: &Path,
) -> LifecycleHistoryMigrationResult<(PathBuf, PathBuf, PathBuf)> {
    let parent = fs::canonicalize(requested.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new(".")))?;
    let name = requested.file_name().ok_or_else(|| {
        LifecycleHistoryMigrationError::Invalid("destination name required".into())
    })?;
    let target = parent.join(name);
    let archive = parent.join(format!(".{}.bradian-g25-source-archive", name.to_string_lossy()));
    let recovered = parent.join(format!(".{}.bradian-g25-source-copy", name.to_string_lossy()));
    let source = fs::canonicalize(source)?;
    for candidate in [&target, &archive, &recovered] {
        if candidate.starts_with(&source) || source.starts_with(candidate) {
            return Err(LifecycleHistoryMigrationError::Invalid(
                "source and migration paths overlap".into(),
            ));
        }
        if fs::symlink_metadata(candidate).is_ok() {
            return Err(LifecycleHistoryMigrationError::Invalid(format!(
                "destination or previous migration artifact exists: {}", candidate.display()
            )));
        }
    }
    Ok((target, archive, recovered))
}

/// Convert a verified G18 logical snapshot to a NEW, independently verified
/// G23 history root. The G19 snapshot is consistent for cooperating local
/// processes. Other writers may resume after the snapshot; there is NO
/// automatic switch-over or replication of changes made after that point.
///
/// This operation requires all catalog tables to be ACTIVE and registered;
/// G23 import checks that requirement and compares every typed row.
/// On failure, temporary G25 artifacts remain for explicit operator cleanup.
pub fn migrate_lifecycle_to_history(
    source_root: impl AsRef<Path>,
    destination_root: impl AsRef<Path>,
) -> LifecycleHistoryMigrationResult<LifecycleHistoryMigrationReport> {
    let source = fs::canonicalize(source_root.as_ref())?;
    if !source.is_dir() || !source.join("catalog").is_dir()
        || !source.join("shared-domain").is_dir()
        || !source.join("lifecycle").is_dir()
        || source.join("history-domain").exists()
    {
        return Err(LifecycleHistoryMigrationError::Invalid(
            "source must be an existing G18 catalog/shared-domain/lifecycle database".into()
        ));
    }
    let (target, archive, recovered) = paths(&source, destination_root.as_ref())?;
    let owner = CatalogLifecycle::open(&source)?;
    // G19 verifies all three archives and the cross-store lifecycle state.
    let captured = owner.backup_database_to(&archive)?;
    drop(owner);

    // G19 validates the restored source image including its lifecycle
    // journal before we intentionally remove that journal from ONLY the
    // temporary copy. The authoritative original retains all three stores.
    let recovered_report = restore_database(&archive, &recovered)?;
    if recovered_report != captured {
        return Err(LifecycleHistoryMigrationError::Invalid(
            "G19 capture and restore receipts disagree".into()
        ));
    }
    {
        let verified = CatalogLifecycle::open(&recovered)?;
        if verified.catalog_backup_identity() !=
                (captured.catalog.last_lsn, captured.catalog.rows)
            || verified.domain_backup_identity() !=
                (captured.domain.last_lsn, captured.domain.rows)
            || verified.lifecycle_backup_identity() !=
                (captured.lifecycle.last_lsn, captured.lifecycle.rows)
            || verified.reconcile()?.len() != captured.active_tables.len()
        {
            return Err(LifecycleHistoryMigrationError::Invalid(
                "verified G18 source copy differs from the captured snapshot".into(),
            ));
        }
    }
    fs::remove_dir_all(recovered.join("lifecycle"))?;

    // The unchanged G23 importer retains both recovered source locks,
    // verifies every row and schema, and publishes only a new history root.
    let imported = migrate_shared_to_history(&recovered, &target)?;
    // The G23 importer independently verifies all typed rows and table pins
    // before publication. The G18 identity check above is performed first.

    // Both paths were created for this call; failures in cleanup do not
    // invalidate the published and independently verified target database.
    let _ = fs::remove_dir_all(&recovered);
    let _ = fs::remove_dir_all(&archive);
    Ok(LifecycleHistoryMigrationReport {
        imported,
        source_lifecycle_lsn: captured.lifecycle.last_lsn,
        captured_catalog_lsn: captured.catalog.last_lsn,
        captured_domain_lsn: captured.domain.last_lsn,
    })
}
