//! ADR 0089 — backup descriptor MAC (BRBKv002).
//!
//! §3.4 checklist: v1 round-trip unchanged, v2 created under
//! auth wiring, v2 verifies under matching key, wrong key
//! rejected, tampered body rejected, missing-auth rejected.

use std::sync::Arc;

use bradian_relational::backup::{read_receipt, read_receipt_with_auth, BackupReceipt};
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

// ---------- §3.4 item 1 — v1 unchanged ----------

#[test]
fn item_1_non_auth_create_produces_v1_descriptor() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let bytes = std::fs::read(bak.join("backup.info")).unwrap();
    assert_eq!(bytes.len(), 40, "v1 descriptor is 40 bytes");
    assert_eq!(&bytes[..8], b"BRBKv001");
}

// ---------- §3.4 item 2 — v2 under auth ----------

#[test]
fn item_2_auth_wiring_create_produces_v2_descriptor() {
    let key = [9u8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "adr-0089"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let bytes = std::fs::read(bak.join("backup.info")).unwrap();
    assert_eq!(bytes.len(), 56, "v2 descriptor is 56 bytes");
    assert_eq!(&bytes[..8], b"BRBKv002");
    // First 36 bytes (body) match v1 shape — only magic differs.
    assert_eq!(u64::from_le_bytes(bytes[8..16].try_into().unwrap()), 1);
    // CRC covers body.
    let crc = u32::from_le_bytes(bytes[36..40].try_into().unwrap());
    assert_eq!(crc, crc32fast::hash(&bytes[..36]));
}

// ---------- §3.4 item 3 — v2 round-trip under matching key ----------

#[test]
fn item_3_v2_round_trips_under_matching_key() {
    let key = [11u8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "adr-0089"))),
        ).unwrap();
        s.put(b"a", b"1").unwrap();
        s.put(b"b", b"2").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let auth = BaceAuthentication::new(key, "adr-0089");
    let r = read_receipt_with_auth(&bak.join("backup.info"), Some(&auth)).unwrap();
    assert_eq!(r.rows, 2);
}

// ---------- §3.4 item 4 — wrong key rejected ----------

#[test]
fn item_4_v2_wrong_key_rejected_with_mac_mismatch() {
    let write_key = [0xAAu8; 32];
    let read_key  = [0xBBu8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(write_key, "w"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let auth = BaceAuthentication::new(read_key, "r");
    let err = read_receipt_with_auth(&bak.join("backup.info"), Some(&auth))
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("MAC") || msg.contains("Corruption"),
        "expected MAC mismatch; got: {msg}");
}

// ---------- §3.4 item 5 — v2 read without auth rejected ----------

#[test]
fn item_5_v2_descriptor_without_auth_rejected() {
    let key = [5u8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let err = read_receipt(&bak.join("backup.info")).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("v2") || msg.contains("authentication") || msg.contains("Corruption"),
        "expected v2-without-auth error; got: {msg}");
}

// ---------- §3.4 item 7 — tampered v2 body rejected ----------

#[test]
fn item_7_tampered_v2_body_rejected() {
    let key = [7u8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w"))),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let mut bytes = std::fs::read(bak.join("backup.info")).unwrap();
    // Flip a byte inside the LSN field (bytes 8..16).
    bytes[12] ^= 0xFF;
    std::fs::write(bak.join("backup.info"), &bytes).unwrap();
    let auth = BaceAuthentication::new(key, "w");
    let err = read_receipt_with_auth(&bak.join("backup.info"), Some(&auth))
        .unwrap_err();
    let msg = format!("{err:?}");
    // CRC catches this first (body changed) — or MAC if CRC somehow happened to match.
    assert!(msg.contains("checksum") || msg.contains("MAC") || msg.contains("Corruption"),
        "expected corruption/MAC error on tampered body; got: {msg}");
}

// ---------- §3.4 item 9 — existing v1 tests unchanged (regression) ----------

#[test]
fn item_9_v1_receipt_still_decodes_under_null_auth() {
    // Hand-craft a BRBKv001 descriptor and verify both the
    // no-auth decode path and the "auth-supplied but descriptor
    // is v1" tolerance.
    let r = BackupReceipt {
        checkpoint_lsn: 3, last_lsn: 7, rows: 5,
        flags: Default::default(),
    };
    let bytes = r.encode();
    assert_eq!(&bytes[..8], b"BRBKv001");
    let a = BackupReceipt::decode(&bytes).unwrap();
    assert_eq!(a, r);
    let auth = BaceAuthentication::new([3u8; 32], "x");
    let b = BackupReceipt::decode_with_auth(&bytes, Some(&auth)).unwrap();
    assert_eq!(b, r);
}
