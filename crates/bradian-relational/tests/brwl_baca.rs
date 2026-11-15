//! ADR 0086 — BRWL BACA carrier (authenticated batch frames).
//!
//! Covers §3.3 items: round-trip under matching key, wrong-key
//! reject, below-threshold opcode-3 preserved, above-threshold
//! opcode-5 emitted, legacy opcode-4 still readable, missing
//! auth wiring rejects opcode-5 cleanly.

use std::sync::Arc;

use bradian_relational::bace::authenticated::BaceAuthentication;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::{
    BaceWiring, RowMutation, RowStore, BRWL_BACE_MIN_BODY_BYTES,
};
use bradian_relational::runtime_limits::StorageLimits;
use tempfile::tempdir;

fn wiring_zstd(auth: Option<BaceAuthentication>) -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: auth,
        strict_authenticated: false,
        previous_authentication: None,
    }
}

fn large_batch(count: usize, value_bytes: usize) -> Vec<RowMutation> {
    (0..count).map(|i| RowMutation::Put {
        key: format!("k{:08}", i).into_bytes(),
        value: vec![b'x'; value_bytes],
    }).collect()
}

fn snapshot(dir: &std::path::Path, bace: Option<BaceWiring>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let store = match bace {
        Some(w) => RowStore::open_with_bace(dir, StorageLimits::default(), w).unwrap(),
        None => panic!("open without wiring would fail on opcode 5 frames"),
    };
    store.scan()
}

// ---------- §3.3 item 1 — below-threshold unchanged ----------

#[test]
fn item_1_small_batch_under_auth_still_opcode_3() {
    let key = [3u8; 32];
    let auth = BaceAuthentication::new(key, "adr-0086");
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_zstd(Some(auth.clone())),
        ).unwrap();
        s.commit_batch(None, &[
            RowMutation::Put { key: b"k".to_vec(), value: b"v".to_vec() },
        ]).unwrap();
    }
    let wal = std::fs::read(dir.path().join("rows.wal")).unwrap();
    let opcode = wal[12 + 8];
    assert_eq!(opcode, 3, "below-threshold batch must stay opcode 3, got {opcode}");
}

// ---------- §3.3 item 2 — above-threshold emits opcode 5 ----------

#[test]
fn item_2_large_batch_under_auth_emits_opcode_5() {
    let key = [7u8; 32];
    let auth = BaceAuthentication::new(key, "adr-0086");
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_zstd(Some(auth.clone())),
        ).unwrap();
        // Craft a batch guaranteed above the threshold.
        let batch = large_batch(50, 32);
        s.commit_batch(None, &batch).unwrap();
    }
    let wal = std::fs::read(dir.path().join("rows.wal")).unwrap();
    // Walk frames looking for opcode 5.
    let mut found_5 = false;
    let mut pos = 0usize;
    while pos < wal.len() {
        let header_end = pos + 12;
        if header_end > wal.len() { break; }
        let body_len = u32::from_le_bytes(wal[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = header_end;
        let body_end = body_start + body_len;
        if body_end > wal.len() { break; }
        let opcode = wal[body_start + 8];
        if opcode == 5 { found_5 = true; }
        pos = body_end;
    }
    assert!(found_5, "above-threshold batch under auth must emit opcode 5");
}

// ---------- §3.3 item 3 — round-trip under matching key ----------

#[test]
fn item_3_opcode_5_round_trips_under_matching_key() {
    let key = [11u8; 32];
    let dir = tempdir().unwrap();
    let batch = large_batch(50, 48);
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_zstd(Some(BaceAuthentication::new(key, "adr-0086"))),
        ).unwrap();
        s.commit_batch(None, &batch).unwrap();
    }
    let rows = snapshot(
        dir.path(),
        Some(wiring_zstd(Some(BaceAuthentication::new(key, "adr-0086")))),
    );
    assert_eq!(rows.len(), 50);
    for (i, (k, v)) in rows.iter().enumerate() {
        assert_eq!(k, &format!("k{:08}", i).into_bytes());
        assert_eq!(v.len(), 48);
    }
}

// ---------- §3.3 item 5 — wrong-key reject ----------

#[test]
fn item_5_wrong_key_reader_rejects_opcode_5() {
    let write_key = [0xAAu8; 32];
    let read_key  = [0xBBu8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_zstd(Some(BaceAuthentication::new(write_key, "write"))),
        ).unwrap();
        s.commit_batch(None, &large_batch(30, 64)).unwrap();
    }
    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring_zstd(Some(BaceAuthentication::new(read_key, "read"))),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("MAC") || msg.contains("Corruption") || msg.contains("BACA"),
        "expected MAC/Corruption/BACA in error; got: {msg}",
    );
}

// ---------- §3.3 item 7 — missing authentication rejects opcode 5 ----------

#[test]
fn item_7_opcode_5_without_authentication_rejects() {
    let key = [5u8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_zstd(Some(BaceAuthentication::new(key, "w"))),
        ).unwrap();
        s.commit_batch(None, &large_batch(25, 40)).unwrap();
    }
    // Reopen with wiring but NO authentication.
    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(), wiring_zstd(None),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        msg.contains("authentication") || msg.contains("BACA") || msg.contains("Corruption"),
        "expected authentication/BACA/Corruption; got: {msg}",
    );
}

// ---------- §3.3 item 8 — opcode-4 legacy frames still read ----------

#[test]
fn item_8_legacy_opcode_4_still_readable() {
    // Write under non-auth wiring (emits opcode 4); read under
    // non-auth wiring (reads opcode 4). This is the pre-ADR-0086
    // behavior and must stay intact.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring_zstd(None),
        ).unwrap();
        s.commit_batch(None, &large_batch(50, 32)).unwrap();
    }
    let rows = snapshot(dir.path(), Some(wiring_zstd(None)));
    assert_eq!(rows.len(), 50);
    // Check opcode 4 is actually what landed on disk.
    let wal = std::fs::read(dir.path().join("rows.wal")).unwrap();
    let mut found_4 = false;
    let mut pos = 0usize;
    while pos < wal.len() {
        let header_end = pos + 12;
        if header_end > wal.len() { break; }
        let body_len = u32::from_le_bytes(wal[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body_start = header_end;
        let body_end = body_start + body_len;
        if body_end > wal.len() { break; }
        if wal[body_start + 8] == 4 { found_4 = true; }
        pos = body_end;
    }
    assert!(found_4, "non-auth wiring must emit opcode 4");
}

// ---------- §3.3 item 10 — MIN_BODY_BYTES constant stays exposed ----------

#[test]
fn item_10_min_body_bytes_constant_reachable() {
    // Constant must be reachable at compile-time via the re-export.
    let _: usize = BRWL_BACE_MIN_BODY_BYTES;
}
