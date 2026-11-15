//! G38 snapshot receipts remain bound to archived versions as source writes advance.
#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::durable_mvcc::DurableMvccStore;
use bradian_relational::history_backup::{
    restore_history_database, restore_history_database_with_bace, HistoryBackupError,
};
use bradian_relational::history_sql::HistorySqlEngine;
use std::fs;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}, mpsc};
use tempfile::tempdir;

#[test]
fn receipt_statistics_describe_the_archive_after_source_mutation() {
    let dir = tempdir().unwrap();
    let source = DurableMvccStore::open(dir.path().join("source")).unwrap();
    let mut tx = source.begin();
    tx.insert(b"key", b"before").unwrap();
    tx.commit().unwrap();
    let (receipt, stats) = source.backup_with_stats(dir.path().join("archive")).unwrap();
    let mut tx = source.begin();
    tx.replace(b"key", b"after").unwrap();
    tx.commit().unwrap();
    assert!(source.latest_lsn() > stats.latest_lsn);
    assert!(source.stats().persisted_versions > stats.persisted_versions);
    DurableMvccStore::restore_backup(dir.path().join("archive"), dir.path().join("restored")).unwrap();
    let restored = DurableMvccStore::open(dir.path().join("restored")).unwrap();
    assert_eq!(restored.latest_lsn(), receipt.last_lsn);
    assert_eq!(restored.stats().persisted_versions, stats.persisted_versions);
    assert_eq!(restored.stats().retention_floor, stats.retention_floor);
    assert_eq!(restored.begin().get(b"key").unwrap(), Some(b"before".to_vec()));
}

#[test]
fn history_archives_remain_self_consistent_with_concurrent_epoch_writes() {
    let (dir, root) = migrated();
    let mut db = HistorySqlEngine::open(&root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    db.execute_ddl("ALTER TABLE bank.public.accounts ADD memo TEXT").unwrap();
    let db = Arc::new(db);
    let done = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel();
    let writer = {
        let db = Arc::clone(&db);
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let mut value = 100;
            while !done.load(Ordering::Acquire) && value < 5000 {
                value += 1;
                modify(&db, &[&format!("UPDATE bank.public.accounts SET balance={value} WHERE id=1")]);
                if value == 110 { ready_tx.send(()).unwrap(); }
                std::thread::yield_now();
            }
        })
    };
    ready_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let mut receipts = Vec::new();
    for i in 0..3 {
        let archive = dir.path().join(format!("archive-{i}"));
        receipts.push((archive.clone(), db.backup_to(&archive).unwrap()));
    }
    done.store(true, Ordering::Release);
    writer.join().unwrap();
    for (i, (archive, report)) in receipts.into_iter().enumerate() {
        let destination = dir.path().join(format!("restored-{i}"));
        restore_history_database(archive, &destination).unwrap();
        let restored = HistorySqlEngine::open(destination).unwrap();
        assert_eq!(restored.last_lsn(), report.history.last_lsn);
        assert_eq!(restored.mvcc_stats().persisted_versions, report.retained_versions);
        assert_eq!(query(&restored, "SELECT balance,memo FROM bank.public.accounts WHERE id=1"),
            query(&db, &format!("SELECT balance,memo FROM bank.public.accounts AS OF {} WHERE id=1", report.history.last_lsn)));
    }
}

// ---------- ADR 0068 — history coordinator backup BACE adapter ----------

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::BaceWiring;

fn wiring_zstd_balanced() -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: None,
        strict_authenticated: false,
    }
}

fn flip_flag_set_bace(backup_info: &std::path::Path) {
    let mut bytes = fs::read(backup_info).unwrap();
    bytes[32] = 0x01;
    let crc = crc32fast::hash(&bytes[..36]);
    bytes[36..40].copy_from_slice(&crc.to_le_bytes());
    fs::write(backup_info, &bytes).unwrap();
}

#[test]
fn restore_history_database_with_bace_on_a_legacy_coordinator_backup_succeeds() {
    // ADR 0068 §3.6 item 2 — wiring is tolerated on all-flag-clear
    // history coordinator backups.
    let (dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let archive = dir.path().join("history-archive");
    let report = db.backup_to(&archive).unwrap();
    for component in ["catalog", "history-domain"] {
        let r = bradian_relational::backup::read_receipt(
            &archive.join(component).join("backup.info"),
        ).unwrap();
        assert!(!r.flags.payload_has_bace_pages);
    }

    let restored_dir = dir.path().join("history-restored");
    restore_history_database_with_bace(&archive, &restored_dir, wiring_zstd_balanced()).unwrap();
    let restored = HistorySqlEngine::open(&restored_dir).unwrap();
    assert_eq!(restored.last_lsn(), report.history.last_lsn);
}

#[test]
fn restore_history_database_legacy_fails_closed_when_sub_archive_declares_bace() {
    // ADR 0068 §3.6 item 5 — history-coordinator mirror of the
    // database-coordinator fail-closed path. The legacy
    // restore_history_database entry point must refuse a coordinator
    // backup where any sub-archive declares BACE pages.
    let (dir, root) = migrated();
    let db = HistorySqlEngine::open(&root).unwrap();
    let archive = dir.path().join("history-archive");
    db.backup_to(&archive).unwrap();

    // Doctor the history-domain sub-archive flag.
    flip_flag_set_bace(&archive.join("history-domain").join("backup.info"));

    let dest = dir.path().join("refused");
    match restore_history_database(&archive, &dest) {
        Err(HistoryBackupError::RestoreRequiresBaceWiring) => {},
        other => panic!("expected RestoreRequiresBaceWiring, got {other:?}"),
    }
    assert!(!dest.exists());
}

// ---------- ADR 0070 — HistorySqlEngine BACE open (end-to-end) ----------

#[test]
fn history_sql_engine_open_with_bace_round_trips_a_legacy_migrated_root() {
    // ADR 0070 §3.6 item 3 — opening with BACE on a directory that
    // was written by the legacy history migration (all v1 pages)
    // works per ADR 0065 §3.5 mixed-generation rule, and SELECT
    // returns the expected rows.
    let (_dir, root) = migrated();
    let db = HistorySqlEngine::open_with_bace(&root, wiring_zstd_balanced()).unwrap();
    let rows = query(&db, "SELECT id,balance FROM bank.public.accounts WHERE id=1");
    assert_eq!(rows.len(), 1);
}

#[test]
fn backup_to_with_bace_emits_flag_set_sub_archives_for_history_coordinator() {
    // ADR 0070 §3.6 items 4 and 8 — backup through a _with_bace-
    // opened engine writes BACE sub-archives, and the honest Q3
    // behaviour: legacy backup_to on a _with_bace-opened engine
    // also produces BACE sub-archives.
    let (dir, root) = migrated();
    let archive = dir.path().join("archive");
    {
        let db = HistorySqlEngine::open_with_bace(&root, wiring_zstd_balanced()).unwrap();
        db.backup_to_with_bace(&archive).unwrap();
    }
    for component in ["catalog", "history-domain"] {
        let r = bradian_relational::backup::read_receipt(
            &archive.join(component).join("backup.info"),
        ).unwrap();
        assert!(
            r.flags.payload_has_bace_pages,
            "{component} sub-archive must carry BACE flag",
        );
    }
    // Same for the legacy coordinator backup entry point on a
    // _with_bace-opened engine.
    let archive_legacy = dir.path().join("archive-legacy-entry");
    {
        let db = HistorySqlEngine::open_with_bace(&root, wiring_zstd_balanced()).unwrap();
        db.backup_to(&archive_legacy).unwrap();
    }
    for component in ["catalog", "history-domain"] {
        let r = bradian_relational::backup::read_receipt(
            &archive_legacy.join(component).join("backup.info"),
        ).unwrap();
        assert!(r.flags.payload_has_bace_pages);
    }
}

#[test]
fn bace_history_backup_round_trips_through_restore_history_database_with_bace() {
    // ADR 0070 §3.6 item 5 — end-to-end G24 BACE loop closes.
    let (dir, root) = migrated();
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    let last_lsn = {
        let db = HistorySqlEngine::open_with_bace(&root, wiring_zstd_balanced()).unwrap();
        db.backup_to_with_bace(&archive).unwrap();
        db.last_lsn()
    };

    restore_history_database_with_bace(&archive, &restored, wiring_zstd_balanced()).unwrap();

    let db = HistorySqlEngine::open_with_bace(&restored, wiring_zstd_balanced()).unwrap();
    assert_eq!(db.last_lsn(), last_lsn);
    let rows = query(&db, "SELECT id,balance FROM bank.public.accounts WHERE id=1");
    assert_eq!(rows.len(), 1);
}

#[test]
fn bace_flag_is_preserved_across_history_coordinator_round_trip() {
    // ADR 0070 §3.6 item 6 adjusted — the drafted test asserted
    // legacy HistorySqlEngine::open on a BACE-restored destination
    // fails closed. Reaching that scenario requires the restored
    // sub-stores to contain actual v2 pages. In the current G24
    // fixture (migrated()), the catalog and history-domain are
    // checkpointed once during migration at a fixed LSN and have
    // no further mutations before `backup_to`. Per the ADR 0065
    // checkpoint-is-idempotent-at-current-LSN rule, backup_to
    // reuses the existing v1 snapshot; no v2 pages are produced.
    //
    // Producing v2 pages through G24 requires either the DDL
    // coordinator (enable_epoch_row_writes + execute_ddl) which
    // ADR 0070 §5 Q2 explicitly defers, or a post-migration row
    // mutation path that also lives behind the DDL coordinator.
    //
    // The still-observable behavior we can assert today: the
    // BackupFlags are preserved in the sub-archive descriptors
    // through backup → restore, so when the DDL coordinator ADR
    // lands and v2 pages are producible, this path is already
    // wired correctly.
    let (dir, root) = migrated();
    let archive = dir.path().join("archive");
    let restored = dir.path().join("restored");
    {
        let db = HistorySqlEngine::open_with_bace(&root, wiring_zstd_balanced()).unwrap();
        db.backup_to_with_bace(&archive).unwrap();
    }
    for component in ["catalog", "history-domain"] {
        let r = bradian_relational::backup::read_receipt(
            &archive.join(component).join("backup.info"),
        ).unwrap();
        assert!(r.flags.payload_has_bace_pages,
            "pre-restore {component} sub-archive must carry BACE flag");
    }
    restore_history_database_with_bace(&archive, &restored, wiring_zstd_balanced()).unwrap();
    for component in ["catalog", "history-domain"] {
        let r = bradian_relational::backup::read_receipt(
            &restored.join(component).join("backup.info"),
        ).unwrap();
        assert!(r.flags.payload_has_bace_pages,
            "post-restore {component} backup.info must still carry BACE flag");
    }
}
