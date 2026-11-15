//! ADR 0093 — BACA key rotation: previous-key fallback.

use std::sync::Arc;

use bradian_relational::backup::{read_receipt_with_rotation, BackupReceipt};
use bradian_relational::bace::authenticated::BaceAuthentication;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::{BaceWiring, RowMutation, RowStore};
use bradian_relational::runtime_limits::StorageLimits;
use tempfile::tempdir;

fn wiring_full(
    primary: Option<BaceAuthentication>,
    previous: Option<BaceAuthentication>,
    strict: bool,
) -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: primary,
        strict_authenticated: strict,
        previous_authentication: previous,
    }
}

// ---------- §3.3 item 2 — primary-key only round-trip ----------

#[test]
fn item_2_primary_key_round_trip_unchanged() {
    let key = [3u8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_full(Some(BaceAuthentication::new(key, "w")), None, false),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let s2 = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring_full(Some(BaceAuthentication::new(key, "w")), None, false),
    ).unwrap();
    assert_eq!(s2.get(b"k"), Some(b"v".to_vec()));
}

// ---------- §3.3 item 3 — rotation fallback reads old-key frames ----------

#[test]
fn item_3_rotation_fallback_reads_previous_key_frames() {
    // Write under key A, then reopen with primary=B / previous=A.
    // All frames MAC under A; reader must fall back to previous.
    let key_a = [0xAAu8; 32];
    let key_b = [0xBBu8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_full(Some(BaceAuthentication::new(key_a, "A")), None, false),
        ).unwrap();
        s.commit_batch(None, &(0..50).map(|i| RowMutation::Put {
            key: format!("k{i:04}").into_bytes(),
            value: vec![b'x'; 40],
        }).collect::<Vec<_>>()).unwrap();
        s.checkpoint().unwrap();
    }
    let s2 = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring_full(
            Some(BaceAuthentication::new(key_b, "B")),
            Some(BaceAuthentication::new(key_a, "A")),
            false,
        ),
    ).unwrap();
    assert_eq!(s2.scan().len(), 50);
}

// ---------- §3.3 item 4 — fallback to wrong previous still fails ----------

#[test]
fn item_4_rotation_with_wrong_previous_key_fails() {
    let key_a = [0xAAu8; 32];
    let key_b = [0xBBu8; 32];
    let key_c = [0xCCu8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_full(Some(BaceAuthentication::new(key_a, "A")), None, false),
        ).unwrap();
        s.commit_batch(None, &(0..40).map(|i| RowMutation::Put {
            key: format!("k{i}").into_bytes(),
            value: vec![b'x'; 40],
        }).collect::<Vec<_>>()).unwrap();
    }
    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring_full(
            Some(BaceAuthentication::new(key_b, "B")),
            Some(BaceAuthentication::new(key_c, "C")),
            false,
        ),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("primary and previous") || msg.contains("MAC"),
        "expected dual-mismatch message; got: {msg}");
}

// ---------- §3.3 item 5 — strict ignores previous ----------

#[test]
fn item_5_strict_mode_ignores_previous_key() {
    let key_a = [0xAAu8; 32];
    let key_b = [0xBBu8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_full(Some(BaceAuthentication::new(key_a, "A")), None, false),
        ).unwrap();
        s.commit_batch(None, &(0..40).map(|i| RowMutation::Put {
            key: format!("k{i}").into_bytes(),
            value: vec![b'x'; 40],
        }).collect::<Vec<_>>()).unwrap();
    }
    // Strict wiring: primary=B, previous=A. Strict ignores previous.
    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring_full(
            Some(BaceAuthentication::new(key_b, "B")),
            Some(BaceAuthentication::new(key_a, "A")),
            true,
        ),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("MAC") || msg.contains("strict"),
        "strict must reject even with correct previous key; got: {msg}");
}

// ---------- §3.3 item 6 — backup descriptor rotation ----------

#[test]
fn item_6_backup_descriptor_rotation_fallback() {
    let key_a = [7u8; 32];
    let key_b = [8u8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring_full(Some(BaceAuthentication::new(key_a, "A")), None, false),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let prim = BaceAuthentication::new(key_b, "B");
    let prev = BaceAuthentication::new(key_a, "A");
    // Reading with primary=B / previous=A must succeed.
    let r = read_receipt_with_rotation(
        &bak.join("backup.info"),
        Some(&prim), Some(&prev), false,
    ).unwrap();
    assert_eq!(r.rows, 1);
}

// ---------- §3.3 item 7 — decode_with_full_rotation direct ----------

#[test]
fn item_7_decode_with_full_rotation_contract() {
    // Hand-craft a v2 descriptor under key A, then verify
    // decode under each wiring shape.
    let key_a = [1u8; 32];
    let key_b = [2u8; 32];
    let a = BaceAuthentication::new(key_a, "A");
    let b = BaceAuthentication::new(key_b, "B");
    let r = BackupReceipt {
        checkpoint_lsn: 1, last_lsn: 1, rows: 0,
        flags: Default::default(),
    };
    let bytes = r.encode_v2(&a);

    // primary=A: ok.
    assert!(BackupReceipt::decode_with_full_rotation(&bytes, Some(&a), None, false).is_ok());
    // primary=B, previous=A, not strict: ok (fallback).
    assert!(BackupReceipt::decode_with_full_rotation(&bytes, Some(&b), Some(&a), false).is_ok());
    // primary=B, previous=A, strict: rejected.
    assert!(BackupReceipt::decode_with_full_rotation(&bytes, Some(&b), Some(&a), true).is_err());
    // primary=B, no previous: rejected.
    assert!(BackupReceipt::decode_with_full_rotation(&bytes, Some(&b), None, false).is_err());
}
