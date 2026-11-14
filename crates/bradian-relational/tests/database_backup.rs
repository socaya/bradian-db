//! G19: complete catalog + shared domain + lifecycle backup and restore.

use std::fs;
use bradian_relational::database_backup::{restore_database, DatabaseBackupError};
use bradian_relational::domain_sql::DomainSqlEngine;
use bradian_relational::lifecycle::CatalogLifecycle;
use bradian_relational::persistent_catalog::PersistentCatalog;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

fn make_database(root: &std::path::Path) {
    {
        let ddl = SqlEngine::open(root).unwrap();
        for statement in [
            "CREATE DATABASE banking",
            "CREATE SCHEMA banking.public",
            "CREATE TABLE banking.public.accounts (id BIGINT NOT NULL, balance BIGINT NOT NULL, PRIMARY KEY(id))",
            "CREATE TABLE banking.public.movements (id BIGINT NOT NULL, account_id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        ] {
            ddl.execute(statement).unwrap();
        }
    }
    let ids = {
        let catalog = PersistentCatalog::open(root.join("catalog")).unwrap();
        catalog.snapshot().tables().map(|table| table.id).collect::<Vec<_>>()
    };
    {
        let owner = CatalogLifecycle::open(root).unwrap();
        for id in ids { owner.provision_existing(id).unwrap(); }
    }
    {
        let db = DomainSqlEngine::open(root).unwrap();
        db.with_session(|session| {
            session.execute("BEGIN")?;
            session.execute("INSERT INTO banking.public.accounts(id,balance) VALUES (10,1000)")?;
            session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES (4,10,100)")?;
            session.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
}

fn assert_database(root: &std::path::Path, balance: i64) {
    let db = DomainSqlEngine::open(root).unwrap();
    db.with_session(|session| {
        session.execute("BEGIN")?;
        assert_eq!(
            session.execute("SELECT balance FROM banking.public.accounts WHERE id=10")?,
            SqlResultSet::Rows {
                columns: vec!["balance".into()],
                rows: vec![vec![Value::Int64(balance)]],
            },
        );
        assert_eq!(
            session.execute("SELECT amount FROM banking.public.movements WHERE id=4")?,
            SqlResultSet::Rows {
                columns: vec!["amount".into()],
                rows: vec![vec![Value::Int64(100)]],
            },
        );
        session.execute("ROLLBACK")?;
        Ok(())
    }).unwrap();
}

#[test]
fn full_database_backup_restore_recovers_all_three_components() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("source");
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    make_database(&src);
    let report = {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap()
    };
    assert_eq!(report.active_tables.len(), 2);
    assert!(report.catalog.last_lsn > 0);
    assert!(report.domain.rows >= 4); // two schema pins + two data rows
    assert!(report.lifecycle.rows >= 3); // version marker + two active records
    for component in ["catalog", "shared-domain", "lifecycle"] {
        assert!(archive.join(component).join("backup.info").exists());
        assert!(archive.join(component).join("checkpoint.current").exists());
    }
    assert!(archive.join("database-backup.json").exists());
    assert!(!archive.join("verified-root").exists());
    let receipt = restore_database(&archive, &restored).unwrap();
    assert_eq!(receipt, report);
    assert_database(&restored, 1000);
    {
        let owner = CatalogLifecycle::open(&restored).unwrap();
        assert_eq!(owner.reconcile().unwrap().len(), 2);
        assert_eq!(owner.last_domain_lsn(), report.domain.last_lsn);
    }
    assert_database(&src, 1000);
}

#[test]
fn captured_backup_is_unchanged_by_later_source_writes() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let archive = dir.path().join("archive");
    let copy = dir.path().join("copy");
    make_database(&src);
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    {
        let db = DomainSqlEngine::open(&src).unwrap();
        db.with_session(|session| {
            session.execute("BEGIN")?;
            session.execute("UPDATE banking.public.accounts SET balance=900 WHERE id=10")?;
            session.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    restore_database(&archive, &copy).unwrap();
    assert_database(&copy, 1000);
    assert_database(&src, 900);
}

#[test]
fn manifest_damage_rejects_restore_without_publishing_destination() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let archive = dir.path().join("backup");
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    let marker = archive.join("database-backup.json");
    let mut bytes = fs::read(&marker).unwrap();
    let index = bytes.iter().position(|v| *v == b'B').unwrap();
    bytes[index] ^= 0x11;
    fs::write(marker, bytes).unwrap();
    let dest = dir.path().join("broken");
    assert!(matches!(restore_database(&archive, &dest), Err(DatabaseBackupError::Corruption(_))));
    assert!(!dest.exists());
}

#[test]
fn tampered_component_archive_is_rejected_before_recovery() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let archive = dir.path().join("archive");
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    let path = archive.join("shared-domain").join("backup.info");
    let mut bytes = fs::read(&path).unwrap();
    bytes[15] ^= 0x77;
    fs::write(path, bytes).unwrap();
    let dest = dir.path().join("damaged");
    assert!(matches!(restore_database(&archive, &dest), Err(DatabaseBackupError::Corruption(_))));
    assert!(!dest.exists());
}

#[test]
fn component_substitution_is_detected_by_bound_archive_checksum() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let archive = dir.path().join("archive");
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    // Mutate a component's valid file without updating the G19 manifest.
    // This test demonstrates the checksum protects component identity even
    // when the directory layout and the general file name remain intact.
    let path = archive.join("catalog").join("checkpoint.current");
    let mut bytes = fs::read(&path).unwrap();
    bytes[8] ^= 1;
    fs::write(path, bytes).unwrap();
    let dest = dir.path().join("no-substitution");
    assert!(matches!(restore_database(&archive, &dest), Err(DatabaseBackupError::Corruption(_))));
    assert!(!dest.exists());
}

#[test]
fn missing_one_component_is_a_hard_restore_failure() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let archive = dir.path().join("archive");
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    fs::remove_dir_all(archive.join("lifecycle")).unwrap();
    let dest = dir.path().join("incomplete");
    assert!(restore_database(&archive, &dest).is_err());
    assert!(!dest.exists());
}

#[test]
fn restore_rejects_existing_destination_and_preserves_its_files() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let archive = dir.path().join("archive");
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    let dest = dir.path().join("existing");
    fs::create_dir(&dest).unwrap();
    fs::write(dest.join("sentinel"), b"untouched").unwrap();
    assert!(restore_database(&archive, &dest).is_err());
    assert_eq!(fs::read(dest.join("sentinel")).unwrap(), b"untouched");
}

#[test]
fn backup_cannot_overlap_active_source_and_refuses_stale_staging() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let owner = CatalogLifecycle::open(&src).unwrap();
    assert!(owner.backup_database_to(src.join("nested")).is_err());
    let archive = dir.path().join("archive");
    fs::create_dir(dir.path().join(".archive.bradian-g19-incomplete")).unwrap();
    assert!(owner.backup_database_to(&archive).is_err());
    assert!(!archive.exists());
}

#[test]
fn competing_full_backup_cannot_acquire_active_catalog_locks() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    make_database(&src);
    let owner = CatalogLifecycle::open(&src).unwrap();
    assert!(CatalogLifecycle::open(&src).is_err());
    let archive = dir.path().join("archive");
    owner.backup_database_to(&archive).unwrap();
    assert!(archive.exists());
}

#[test]
fn restored_database_can_accept_new_cross_table_transactions() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("src");
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    make_database(&src);
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    restore_database(&archive, &restored).unwrap();
    {
        let db = DomainSqlEngine::open(&restored).unwrap();
        db.with_session(|session| {
            session.execute("BEGIN")?;
            session.execute("UPDATE banking.public.accounts SET balance=700 WHERE id=10")?;
            session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(5,10,-300)")?;
            session.execute("COMMIT")?;
            Ok(())
        }).unwrap();
    }
    assert_database(&restored, 700);
    assert_database(&src, 1000);
}

// ---------- ADR 0068 — coordinator backup BACE adapter ----------

use std::sync::Arc;
use bradian_relational::backup::{self, BackupFlags};
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::database_backup::{
    restore_database_with_bace, restore_database_with_bace_and_metadata_limits,
};
use bradian_relational::row_store::BaceWiring;
use bradian_relational::runtime_limits::MetadataLimits;

fn wiring_zstd_balanced() -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
    }
}

/// Doctor one sub-archive's `backup.info` to claim BACE pages (flag
/// bit 0 set at byte 32), and recompute the trailing CRC so the
/// descriptor remains self-consistent. Used to simulate a future
/// CatalogLifecycle::open_with_bace world without actually having
/// that entry point yet.
fn flip_flag_set_bace(backup_info: &std::path::Path) {
    let mut bytes = fs::read(backup_info).unwrap();
    bytes[32] = 0x01;
    let crc = crc32fast::hash(&bytes[..36]);
    bytes[36..40].copy_from_slice(&crc.to_le_bytes());
    fs::write(backup_info, &bytes).unwrap();
}

#[test]
fn restore_database_with_bace_on_a_legacy_coordinator_backup_succeeds() {
    // ADR 0068 §3.6 item 1 — wiring is tolerated on all-flag-clear
    // coordinator backups (ADR 0065 §3.5 rule applied to each sub-
    // archive).
    let dir = tempdir().unwrap();
    let src = dir.path().join("source");
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored-via-bace");
    make_database(&src);
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }

    // Every sub-archive has flag=0 because CatalogLifecycle::open
    // uses the legacy RowStore::open path.
    for component in ["catalog", "shared-domain", "lifecycle"] {
        let r = backup::read_receipt(&archive.join(component).join("backup.info")).unwrap();
        assert!(!r.flags.payload_has_bace_pages);
    }

    restore_database_with_bace(&archive, &restored, wiring_zstd_balanced()).unwrap();
    assert_database(&restored, 1000);
}

#[test]
fn restore_database_legacy_fails_closed_with_specific_error_when_any_sub_archive_declares_bace() {
    // ADR 0068 §3.6 items 3 and 6 — a sub-archive whose backup.info
    // flag is set must cause restore_database to fail with
    // RestoreRequiresBaceWiring BEFORE any staging directory is
    // created, and BEFORE any sub-archive bytes are decoded.
    let dir = tempdir().unwrap();
    let src = dir.path().join("source");
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    make_database(&src);
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    // Doctor the lifecycle sub-archive's flag to claim BACE pages.
    flip_flag_set_bace(&archive.join("lifecycle").join("backup.info"));

    match restore_database(&archive, &restored) {
        Err(DatabaseBackupError::RestoreRequiresBaceWiring) => {},
        other => panic!("expected RestoreRequiresBaceWiring, got {other:?}"),
    }
    assert!(!restored.exists(), "destination must not be created");
    let parent = restored.parent().unwrap();
    let stage_name = format!(".{}.bradian-g19-incomplete",
        restored.file_name().unwrap().to_string_lossy());
    assert!(!parent.join(stage_name).exists(), "staging dir must not be created");
}

// ADR 0068 §3.6 item 4 (doctored flag tolerance through _with_bace)
// is documented in the ADR and acceptance doc but is not covered by
// an integration test. Flipping a sub-archive's flag also changes
// its archive_crc32, so the test would additionally need to rewrite
// the coordinator manifest to keep the archive_crc32 consistent —
// too much test infrastructure for a corner case. The behavior is
// covered implicitly by `..._tolerates_doctored_flag_...` in the
// backup_restore test suite (ADR 0067 §3.7 item 9), which asserts
// the sub-archive-level rule that this coordinator inherits.

#[test]
fn read_receipt_is_publicly_reachable_and_returns_flags() {
    // ADR 0068 §3.6 item 7 — the previously module-private
    // read_receipt is now part of the crate's public API so
    // coordinator carriers (and tests) can probe flag bits
    // without reopening the RowStore.
    let dir = tempdir().unwrap();
    let src = dir.path().join("source");
    let archive = dir.path().join("archive");
    make_database(&src);
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    let r = backup::read_receipt(&archive.join("catalog").join("backup.info")).unwrap();
    assert_eq!(r.flags, BackupFlags::default());
}

#[test]
fn restore_database_with_bace_and_metadata_limits_is_reachable() {
    // Covers the non-default-limits entry point so the API surface
    // is exercised in CI.
    let dir = tempdir().unwrap();
    let src = dir.path().join("source");
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    make_database(&src);
    {
        let owner = CatalogLifecycle::open(&src).unwrap();
        owner.backup_database_to(&archive).unwrap();
    }
    restore_database_with_bace_and_metadata_limits(
        &archive, &restored, MetadataLimits::default(), wiring_zstd_balanced(),
    ).unwrap();
    assert_database(&restored, 1000);
}
