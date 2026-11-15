//! End-to-end consistent backup and restore against actual Bradian WAL files.

use std::fs;
use std::sync::Arc;
use std::thread;

use bradian_relational::backup::BackupReceipt;
use bradian_relational::row_store::{RowMutation, RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn complete_backup_and_restore_roundtrip_with_checkpoint_and_wal() {
    let base = tempdir().unwrap();
    let db = base.path().join("primary");
    let backup = base.path().join("backup-20261112");
    let restored = base.path().join("restored");
    let report;
    {
        let store = RowStore::open(&db).unwrap();
        store.put(b"alice", b"active").unwrap();
        store.put(b"bob", b"inactive").unwrap();
        store.commit_batch(None, &[
            RowMutation::Put { key: b"charlie".to_vec(), value: b"new".to_vec() },
            RowMutation::Delete { key: b"bob".to_vec() },
        ]).unwrap();
        report = store.backup_to(&backup).unwrap();
        assert_eq!(report.rows, 2);
        assert_eq!(report.last_lsn, 3);
        assert_eq!(report.checkpoint_lsn, 3);
        assert!(backup.join("backup.info").is_file());
        assert!(backup.join("checkpoint.current").is_file());
        assert!(backup.join("snapshot-3.brp").is_file());
        assert!(backup.join("index-3.bri").is_file());
        assert!(backup.join("rows.wal").is_file());
        assert!(!backup.join("snapshot-3.tmp").exists());
    }
    let result = RowStore::restore_backup(&backup, &restored).unwrap();
    assert_eq!(result, report);
    let recovered = RowStore::open(&restored).unwrap();
    assert_eq!(recovered.last_lsn(), 3);
    assert_eq!(recovered.get(b"alice"), Some(b"active".to_vec()));
    assert_eq!(recovered.get(b"bob"), None);
    assert_eq!(recovered.get(b"charlie"), Some(b"new".to_vec()));
    assert_eq!(recovered.verify_checkpoint_index().unwrap().rows, 2);
    recovered.put(b"later", b"restored-write").unwrap();
    assert_eq!(recovered.last_lsn(), 4);
}

#[test]
fn source_changes_after_backup_do_not_change_captured_state() {
    let base = tempdir().unwrap();
    let source = base.path().join("db");
    let backup = base.path().join("frozen");
    let restored = base.path().join("restore");
    let store = RowStore::open(&source).unwrap();
    store.put(b"key", b"before").unwrap();
    let receipt = store.backup_to(&backup).unwrap();
    store.put(b"key", b"after").unwrap();
    store.put(b"new", b"not-in-backup").unwrap();
    RowStore::restore_backup(&backup, &restored).unwrap();
    let recovered = RowStore::open(&restored).unwrap();
    assert_eq!(recovered.last_lsn(), receipt.last_lsn);
    assert_eq!(recovered.get(b"key"), Some(b"before".to_vec()));
    assert_eq!(recovered.get(b"new"), None);
}

#[test]
fn backup_preserves_checkpoint_plus_subsequent_wal_frames() {
    let base = tempdir().unwrap();
    let root = base.path().join("source");
    let backup = base.path().join("captured");
    let restored = base.path().join("restored");
    let store = RowStore::open(&root).unwrap();
    store.put(b"base", b"first").unwrap();
    store.checkpoint().unwrap();
    store.put(b"late", b"after-checkpoint").unwrap();
    store.put(b"extra", b"also-in-WAL").unwrap();
    let report = store.backup_current_generation(&backup).unwrap();
    assert_eq!(report.checkpoint_lsn, 1);
    assert_eq!(report.last_lsn, 3);
    assert_eq!(report.rows, 3);
    assert!(fs::metadata(backup.join("rows.wal")).unwrap().len() > 0);
    RowStore::restore_backup(&backup, &restored).unwrap();
    let recovered = RowStore::open(&restored).unwrap();
    assert_eq!(recovered.get(b"base"), Some(b"first".to_vec()));
    assert_eq!(recovered.get(b"late"), Some(b"after-checkpoint".to_vec()));
    assert_eq!(recovered.get(b"extra"), Some(b"also-in-WAL".to_vec()));
    assert_eq!(recovered.last_lsn(), 3);
}

#[test]
fn restoring_refuses_existing_destination_and_keeps_it_unchanged() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let existing = base.path().join("existing");
    let store = RowStore::open(&db).unwrap();
    store.put(b"data", b"original").unwrap();
    store.backup_to(&backup).unwrap();
    fs::create_dir(&existing).unwrap();
    fs::write(existing.join("sentinel"), b"must-stay").unwrap();
    assert!(RowStore::restore_backup(&backup, &existing).is_err());
    assert_eq!(fs::read(existing.join("sentinel")).unwrap(), b"must-stay");
    assert!(store.backup_to(&backup).is_err());
}

#[test]
fn corrupted_backup_manifest_or_wal_cannot_be_restored() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"data", b"present").unwrap();
    store.backup_to(&backup).unwrap();

    let mut descriptor = fs::read(backup.join("backup.info")).unwrap();
    descriptor[12] ^= 0x23;
    fs::write(backup.join("backup.info"), descriptor).unwrap();
    assert!(RowStore::restore_backup(&backup, base.path().join("invalid")).is_err());
}

#[test]
fn corrupted_snapshot_backup_fails_even_when_descriptor_is_valid() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"data", b"present").unwrap();
    store.backup_to(&backup).unwrap();
    let snapshot = backup.join("snapshot-1.brp");
    let mut bytes = fs::read(&snapshot).unwrap();
    bytes[4096 + 55] ^= 0xff;
    fs::write(snapshot, bytes).unwrap();
    let restored = base.path().join("restore");
    assert!(RowStore::restore_backup(&backup, &restored).is_err());
    assert!(!restored.exists(), "no verified directory should be published");
}

#[test]
fn metadata_format_roundtrip_rejects_lsn_regressions() {
    let receipt = BackupReceipt {
        checkpoint_lsn: 21, last_lsn: 42, rows: 8,
        flags: Default::default(),
    };
    let bytes = receipt.encode();
    assert_eq!(bytes.len(), 40);
    assert_eq!(BackupReceipt::decode(&bytes).unwrap(), receipt);
    let invalid = BackupReceipt {
        checkpoint_lsn: 9, last_lsn: 8, rows: 1,
        flags: Default::default(),
    };
    assert!(BackupReceipt::decode(&invalid.encode()).is_err());
}

#[test]
fn backup_directory_inside_active_database_is_rejected() {
    let base = tempdir().unwrap();
    let root = base.path().join("db");
    let store = RowStore::open(&root).unwrap();
    store.put(b"row", b"safe").unwrap();
    assert!(matches!(
        store.backup_to(root.join("nested-backup")),
        Err(StoreError::Corruption(_))
    ));
    assert_eq!(store.get(b"row"), Some(b"safe".to_vec()));
}

#[test]
fn verified_backup_with_parallel_writers_contains_one_committed_state() {
    let base = tempdir().unwrap();
    let source = Arc::new(RowStore::open(base.path().join("source")).unwrap());
    for i in 0..50 {
        source.put(format!("row-{i:03}").as_bytes(), b"initial").unwrap();
    }
    let writer = {
        let store = Arc::clone(&source);
        thread::spawn(move || {
            for i in 0..30 {
                store.put(format!("row-{i:03}").as_bytes(), b"updated").unwrap();
            }
        })
    };
    let backup = base.path().join("consistent");
    let report = source.backup_to(&backup).unwrap();
    writer.join().unwrap();
    let restored = base.path().join("restored");
    RowStore::restore_backup(&backup, &restored).unwrap();
    let copy = RowStore::open(&restored).unwrap();
    assert_eq!(copy.last_lsn(), report.last_lsn);
    assert_eq!(copy.len(), 50);
    // Writer modifies rows in key order; a consistent backup must contain
    // a prefix of the updates, never a partial WAL record or corrupt row.
    let updated = copy.scan().into_iter().filter(|(_,v)|v == b"updated").count();
    assert!(updated <= 30);
    for i in 0..updated {
        assert_eq!(copy.get(format!("row-{i:03}").as_bytes()), Some(b"updated".to_vec()));
    }
}

#[test]
fn corrupted_post_checkpoint_wal_is_rejected_during_restore() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"base", b"checkpointed").unwrap();
    store.checkpoint().unwrap();
    store.put(b"after", b"wal-only").unwrap();
    store.backup_current_generation(&backup).unwrap();

    let path = backup.join("rows.wal");
    let mut bytes = fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(path, bytes).unwrap();

    let restored = base.path().join("restored");
    assert!(RowStore::restore_backup(&backup, &restored).is_err());
    assert!(!restored.exists());
}

#[test]
fn current_generation_backup_requires_committed_checkpoint() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let store = RowStore::open(&db).unwrap();
    store.put(b"key", b"value").unwrap();
    let dest = base.path().join("backup");
    assert!(store.backup_current_generation(&dest).is_err());
    assert!(!dest.exists());
    assert_eq!(store.last_lsn(), 1);
}


#[test]
fn truncated_extra_wal_frame_must_not_be_silently_repaired_during_restore() {
    let base = tempdir().unwrap();
    let db = base.path().join("db");
    let backup = base.path().join("backup");
    let store = RowStore::open(&db).unwrap();
    store.put(b"before", b"checkpoint").unwrap();
    store.checkpoint().unwrap();
    store.put(b"after", b"committed").unwrap();
    store.backup_current_generation(&backup).unwrap();

    // A partial trailing record is acceptable during ordinary crash
    // recovery, but an immutable backup must be an exact captured object.
    let path = backup.join("rows.wal");
    let mut bytes = fs::read(&path).unwrap();
    bytes.extend_from_slice(b"BRWL\x01"); // truncated next frame header
    fs::write(&path, bytes).unwrap();

    let restored = base.path().join("restore");
    assert!(RowStore::restore_backup(&backup, &restored).is_err());
    assert!(!restored.exists());
}

#[test]
fn backup_into_legacy_checkpoint_without_manifest_is_migrated_safely() {
    let base = tempdir().unwrap();
    let db = base.path().join("legacy-db");
    {
        let store = RowStore::open(&db).unwrap();
        store.put(b"legacy", b"kept").unwrap();
        store.checkpoint().unwrap();
    }
    fs::remove_file(db.join("checkpoint.current")).unwrap();
    let store = RowStore::open(&db).unwrap();
    let backup = base.path().join("legacy-backup");
    let receipt = store.backup_to(&backup).unwrap();
    assert_eq!(receipt.checkpoint_lsn, 1);
    assert_eq!(receipt.last_lsn, 1);
    assert!(db.join("checkpoint.current").exists());

    let restored = base.path().join("legacy-restored");
    RowStore::restore_backup(&backup, &restored).unwrap();
    assert_eq!(RowStore::open(&restored).unwrap().get(b"legacy"), Some(b"kept".to_vec()));
}

// ---------- ADR 0067 — backup carrier BACE adapter ----------

use bradian_relational::backup::BackupFlags;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::BaceWiring;
use bradian_relational::runtime_limits::StorageLimits;

fn wiring_with_profile(profile: CompressionProfile) -> BaceWiring {
    let baseline = PlatformBaseline { profile, ..PlatformBaseline::shipped() };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: None,
        strict_authenticated: false,
    }
}

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

#[test]
fn backup_receipt_roundtrip_with_flag_clear_yields_canonical_zero_flag_bytes() {
    // ADR 0067 §3.7 item 1 — encode/decode preserves flag=false and
    // the on-disk flag bytes are all zero.
    let r = BackupReceipt {
        checkpoint_lsn: 5, last_lsn: 7, rows: 2,
        flags: BackupFlags { payload_has_bace_pages: false },
    };
    let bytes = r.encode();
    assert_eq!(&bytes[32..36], &[0, 0, 0, 0], "flag bytes must be zero");
    assert_eq!(BackupReceipt::decode(&bytes).unwrap(), r);
}

#[test]
fn backup_receipt_roundtrip_with_flag_set_yields_bit_zero_at_byte_32() {
    // ADR 0067 §3.7 item 2 — encode/decode preserves flag=true and
    // the on-disk flag bytes are exactly {0x01, 0x00, 0x00, 0x00}.
    let r = BackupReceipt {
        checkpoint_lsn: 5, last_lsn: 7, rows: 2,
        flags: BackupFlags { payload_has_bace_pages: true },
    };
    let bytes = r.encode();
    assert_eq!(&bytes[32..36], &[1, 0, 0, 0], "only bit 0 set, little-endian");
    assert_eq!(BackupReceipt::decode(&bytes).unwrap(), r);
}

#[test]
fn backup_receipt_decode_rejects_non_zero_reserved_flag_bits() {
    // ADR 0067 §3.7 item 3 — reserved bit 1 non-zero must fail
    // closed with Corruption, even if CRC is recomputed correctly.
    let r = BackupReceipt {
        checkpoint_lsn: 5, last_lsn: 7, rows: 2,
        flags: BackupFlags { payload_has_bace_pages: false },
    };
    let mut bytes = r.encode();
    // Set reserved bit 1 and recompute CRC so the only failure is the
    // reserved-bit check.
    bytes[32] = 0x02;
    let crc = crc32fast::hash(&bytes[..36]);
    bytes[36..40].copy_from_slice(&crc.to_le_bytes());
    let err = BackupReceipt::decode(&bytes).unwrap_err();
    assert!(matches!(err, StoreError::Corruption(msg) if msg.contains("reserved backup flag bits")),
        "expected reserved-flag Corruption");
}

#[test]
fn backup_receipt_decode_rejects_flag_byte_tamper_without_crc_update() {
    // ADR 0067 §3.7 item 4 — if someone twiddles the flag byte on a
    // legacy descriptor without updating the CRC, decode must fail
    // with the CRC-mismatch error.
    let r = BackupReceipt {
        checkpoint_lsn: 5, last_lsn: 7, rows: 2,
        flags: BackupFlags { payload_has_bace_pages: false },
    };
    let mut bytes = r.encode();
    bytes[32] = 0x01; // set bit 0; leave CRC bytes stale
    let err = BackupReceipt::decode(&bytes).unwrap_err();
    assert!(matches!(err, StoreError::Corruption(msg) if msg.contains("checksum mismatch")),
        "expected CRC-mismatch Corruption");
}

#[test]
fn bace_raw_profile_backup_has_flag_clear_and_restores_through_legacy_path() {
    // ADR 0067 §3.7 item 5 — Raw-profile BACE-opt-in store writes v1
    // pages. backup_current_generation produces a flag-clear receipt
    // and the legacy restore path (no wiring) works.
    let base = tempdir().unwrap();
    let db = base.path().join("src");
    let backup_path = base.path().join("backup-raw");
    let restored = base.path().join("restored-raw");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring_with_profile(CompressionProfile::Raw),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&backup_path).unwrap();
    }

    let receipt = BackupReceipt::decode(
        &fs::read(backup_path.join("backup.info")).unwrap(),
    ).unwrap();
    assert!(!receipt.flags.payload_has_bace_pages,
        "Raw-profile backup must have flag=false");

    RowStore::restore_backup(&backup_path, &restored).unwrap();
    assert_eq!(RowStore::open(&restored).unwrap().get(b"k"), Some(b"v".to_vec()));
}

#[test]
fn bace_balanced_backup_has_flag_set_and_legacy_restore_fails_closed() {
    // ADR 0067 §3.7 item 6 — Balanced-profile BACE-opt-in store writes
    // v2 pages. Backup receipt has flag=true. Legacy restore_backup
    // must fail closed with BackupRestoreRequiresBaceWiring BEFORE
    // touching the destination.
    let base = tempdir().unwrap();
    let db = base.path().join("src");
    let backup_path = base.path().join("backup-bace");
    let restored = base.path().join("restored-bace");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&backup_path).unwrap();
    }

    let receipt = BackupReceipt::decode(
        &fs::read(backup_path.join("backup.info")).unwrap(),
    ).unwrap();
    assert!(receipt.flags.payload_has_bace_pages,
        "Balanced-profile backup must have flag=true");

    match RowStore::restore_backup(&backup_path, &restored) {
        Err(StoreError::BackupRestoreRequiresBaceWiring) => {},
        other => panic!("expected BackupRestoreRequiresBaceWiring, got {other:?}"),
    }
    assert!(!restored.exists(), "destination must not be created on fail-closed");
}

#[test]
fn bace_balanced_backup_restores_through_with_bace_entry_point() {
    // ADR 0067 §3.7 item 7 — the matching restore_backup_with_bace
    // call succeeds and every row round-trips.
    let base = tempdir().unwrap();
    let db = base.path().join("src");
    let backup_path = base.path().join("backup-bace");
    let restored = base.path().join("restored-bace");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        for i in 0..5 {
            s.put(format!("k{i}").as_bytes(), format!("v{i}").as_bytes()).unwrap();
        }
        s.backup_to(&backup_path).unwrap();
    }

    RowStore::restore_backup_with_bace(
        &backup_path, &restored, StorageLimits::default(),
        wiring_zstd_balanced(),
    ).unwrap();

    let reopened = RowStore::open_with_bace(
        &restored, StorageLimits::default(), wiring_zstd_balanced(),
    ).unwrap();
    for i in 0..5 {
        assert_eq!(
            reopened.get(format!("k{i}").as_bytes()),
            Some(format!("v{i}").as_bytes().to_vec()),
        );
    }
}

#[test]
fn bace_backup_restore_fails_closed_when_wiring_lacks_source_codec() {
    // ADR 0067 §3.7 item 8 — restoring a ZSTD-wrapped backup with
    // a wiring whose registry does not know ZSTD must fail closed
    // during verify-time recovery.
    let base = tempdir().unwrap();
    let db = base.path().join("src");
    let backup_path = base.path().join("backup-zstd");
    let restored = base.path().join("restored-zstd");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&backup_path).unwrap();
    }

    // Build a wiring whose registry has only RAW (no ZSTD) but
    // declares Balanced profile. The registry.codec(ZSTD_ID) returns
    // None, so recovery must fire BaceCodecUnavailable.
    let registry_raw_only = Arc::new(
        BuiltInRegistry::with_codecs(vec![Box::new(
            bradian_relational::bace::codec::raw::RawCodec,
        )]),
    );
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![RAW_ID],
        ..PlatformBaseline::shipped()
    };
    let policy_svc = Arc::new(InMemoryPolicyService::new(
        baseline, BuiltInRegistry::new(),
    ));
    let wiring_missing_zstd = BaceWiring {
        policy_service: policy_svc,
        registry: registry_raw_only,
        authentication: None,
        strict_authenticated: false,
    };

    let err = RowStore::restore_backup_with_bace(
        &backup_path, &restored, StorageLimits::default(),
        wiring_missing_zstd,
    ).unwrap_err();
    assert!(
        matches!(err, StoreError::BaceCodecUnavailable { codec_id } if codec_id == ZSTD_ID),
        "expected BaceCodecUnavailable(ZSTD_ID), got {err:?}",
    );
    assert!(!restored.exists(), "destination must not be published on fail-closed");
}

#[test]
fn legacy_pre_bace_backup_restores_through_with_bace_entry_point() {
    // ADR 0067 §3.7 item 9 — a legacy backup (created by a non-BACE
    // RowStore, flag=0) is restorable through restore_backup_with_bace.
    // ADR 0065 §3.5 guarantees open_with_bace works on v1-only.
    let base = tempdir().unwrap();
    let db = base.path().join("src");
    let backup_path = base.path().join("backup-legacy");
    let restored = base.path().join("restored-via-bace");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"legacy", b"ok").unwrap();
        s.backup_to(&backup_path).unwrap();
    }

    let receipt = BackupReceipt::decode(
        &fs::read(backup_path.join("backup.info")).unwrap(),
    ).unwrap();
    assert!(!receipt.flags.payload_has_bace_pages);

    RowStore::restore_backup_with_bace(
        &backup_path, &restored, StorageLimits::default(),
        wiring_zstd_balanced(),
    ).unwrap();
    assert_eq!(RowStore::open(&restored).unwrap().get(b"legacy"), Some(b"ok".to_vec()));
}

#[test]
fn legacy_restore_fails_closed_on_tampered_flag_set_backup_before_touching_destination() {
    // ADR 0067 §3.7 item 10 (revised) — item 10 in the ADR §3.7 named
    // source-side behavior on an unreachable code path (the public
    // backup_current_generation now always threads self.bace_wiring
    // through, so there is no public way to bypass wiring on the
    // write side). The still-observable failure mode is on the READ
    // side: legacy restore_backup must fail closed BEFORE creating
    // any staging directory when the receipt declares BACE pages.
    //
    // Prior test `bace_balanced_backup_has_flag_set_and_legacy_restore_fails_closed`
    // covered the happy-path flag=true case. Here we doctor a
    // legacy-pre-ADR-0067 backup to have flag=true with correct CRC
    // and confirm the pre-destination fail-closed path holds.
    let base = tempdir().unwrap();
    let db = base.path().join("src");
    let backup_path = base.path().join("backup-doctored");
    let restored = base.path().join("restored-doctored");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&backup_path).unwrap();
    }

    // Doctor backup.info to claim flag=true while keeping the CRC
    // valid. There are no BACE pages on disk; the test observes that
    // the legacy path refuses to touch the destination.
    let info_path = backup_path.join("backup.info");
    let mut bytes = fs::read(&info_path).unwrap();
    bytes[32] = 0x01;
    let crc = crc32fast::hash(&bytes[..36]);
    bytes[36..40].copy_from_slice(&crc.to_le_bytes());
    fs::write(&info_path, &bytes).unwrap();

    match RowStore::restore_backup(&backup_path, &restored) {
        Err(StoreError::BackupRestoreRequiresBaceWiring) => {},
        other => panic!("expected BackupRestoreRequiresBaceWiring, got {other:?}"),
    }
    assert!(!restored.exists(), "destination must not be created");
}
