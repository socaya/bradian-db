//! ADR 0090 — BRPG v2 BACA checkpoint pages.
//!
//! §3.4 checklist: no-auth wiring still emits bit-0 BACE pages;
//! auth wiring emits bit-1 BACA pages; round-trip under matching
//! key; wrong-key MAC rejected; missing-auth on BACA page
//! rejected; both-bits-set page rejected.

use std::sync::Arc;

use bradian_relational::bace::authenticated::BaceAuthentication;
use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::{BaceWiring, RowStore};
use bradian_relational::runtime_limits::StorageLimits;
use tempfile::tempdir;

fn wiring(auth: Option<BaceAuthentication>) -> BaceWiring {
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
    }
}

fn find_page_flag_byte(path: &std::path::Path, page_index: u64) -> u8 {
    // BRPG v2 page layout: 4096 bytes per page; feature-flags byte at [7].
    let bytes = std::fs::read(path).unwrap();
    let off = (page_index as usize) * 4096;
    bytes[off + 7]
}

// ---------- §3.4 item 2 — auth wiring emits bit-1 BACA pages ----------

#[test]
fn item_2_auth_wiring_emits_baca_pages_bit_1() {
    let key = [11u8; 32];
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("db");
    {
        let s = RowStore::open_with_bace(
            &db_path, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "adr-0090"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let snap = db_path.join("snapshot-1.brp");
    // Page 0 is metadata (v1), page 1 is the first data page.
    let flag = find_page_flag_byte(&snap, 1);
    assert_eq!(flag & 0b11, 0b10, "page 1 should have bit 1 (BACA), not bit 0 (BACE); got {flag:#010b}");
}

// ---------- §3.4 item 3 — round-trip under matching key ----------

#[test]
fn item_3_round_trip_under_matching_key() {
    let key = [7u8; 32];
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("db");
    {
        let s = RowStore::open_with_bace(
            &db_path, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w"))),
        ).unwrap();
        s.put(b"alice", b"active").unwrap();
        s.put(b"bob", b"inactive").unwrap();
        s.checkpoint().unwrap();
    }
    let s2 = RowStore::open_with_bace(
        &db_path, StorageLimits::default(),
        wiring(Some(BaceAuthentication::new(key, "w"))),
    ).unwrap();
    assert_eq!(s2.get(b"alice"), Some(b"active".to_vec()));
    assert_eq!(s2.get(b"bob"), Some(b"inactive".to_vec()));
}

// ---------- §3.4 item 4 — wrong key rejected ----------

#[test]
fn item_4_wrong_key_reader_rejects_baca_page() {
    let write_key = [0xAAu8; 32];
    let read_key  = [0xBBu8; 32];
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("db");
    {
        let s = RowStore::open_with_bace(
            &db_path, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(write_key, "w"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let err = RowStore::open_with_bace(
        &db_path, StorageLimits::default(),
        wiring(Some(BaceAuthentication::new(read_key, "r"))),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("MAC") || msg.contains("Corruption") || msg.contains("BACA"),
        "expected MAC error; got: {msg}");
}

// ---------- §3.4 item 5 — missing auth rejects BACA page ----------

#[test]
fn item_5_missing_auth_rejects_baca_page() {
    let key = [3u8; 32];
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("db");
    {
        let s = RowStore::open_with_bace(
            &db_path, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let err = RowStore::open_with_bace(
        &db_path, StorageLimits::default(), wiring(None),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("authentication") || msg.contains("BACA") || msg.contains("Corruption"),
        "expected auth-required error; got: {msg}");
}

// ---------- §3.4 item 1 — no-auth wiring still emits bit-0 BACE ----------

#[test]
fn item_1_no_auth_wiring_still_emits_bace_pages_bit_0() {
    let dir = tempdir().unwrap();
    let db_path = dir.path().join("db");
    {
        let s = RowStore::open_with_bace(
            &db_path, StorageLimits::default(), wiring(None),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let snap = db_path.join("snapshot-1.brp");
    let flag = find_page_flag_byte(&snap, 1);
    assert_eq!(flag & 0b11, 0b01, "page 1 should have bit 0 (BACE); got {flag:#010b}");
}
