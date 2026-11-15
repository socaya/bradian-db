//! ADR 0092 — strict-authenticated mode on BRWL + BRPG carriers.
//! (`.brcs` strict mode is in bradian-storage tests.)

use std::sync::Arc;

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

fn wiring(auth: Option<BaceAuthentication>, strict: bool) -> BaceWiring {
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![ZSTD_ID, RAW_ID],
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(baseline, BuiltInRegistry::new())),
        registry: Arc::new(BuiltInRegistry::new()),
        authentication: auth,
        strict_authenticated: strict,
    }
}

fn large_batch(count: usize, value_bytes: usize) -> Vec<RowMutation> {
    (0..count).map(|i| RowMutation::Put {
        key: format!("k{:08}", i).into_bytes(),
        value: vec![b'x'; value_bytes],
    }).collect()
}

// ---------- §3 item 5 — BRWL strict rejects opcode 4 ----------

#[test]
fn item_5_brwl_strict_rejects_opcode_4() {
    // Write batches under NON-auth wiring (emits opcode 4),
    // then reopen with strict + auth (must reject opcode 4).
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring(None, false),
        ).unwrap();
        s.commit_batch(None, &large_batch(50, 32)).unwrap();
    }
    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring(Some(BaceAuthentication::new([3u8; 32], "k")), true),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("strict mode"), "expected strict rejection; got {msg}");
}

// ---------- §3 item 6 — BRWL strict accepts opcode 5 ----------

#[test]
fn item_6_brwl_strict_accepts_opcode_5() {
    let key = [11u8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w")), false),
        ).unwrap();
        s.commit_batch(None, &large_batch(50, 32)).unwrap();
    }
    // Reopen with strict=true; opcode-5 frames must be accepted.
    let s2 = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring(Some(BaceAuthentication::new(key, "w")), true),
    ).unwrap();
    assert_eq!(s2.scan().len(), 50);
}

// ---------- §3 item 7 — BRPG strict rejects bit-0 pages ----------

#[test]
fn item_7_brpg_strict_rejects_bit_0_page() {
    let dir = tempdir().unwrap();
    {
        // Non-auth wiring with ZSTD → emits bit-0 (BACE) pages.
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring(None, false),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring(Some(BaceAuthentication::new([7u8; 32], "k")), true),
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("strict mode"), "expected strict rejection; got {msg}");
}

// ---------- §3 item 8 — BRPG strict accepts bit-1 pages ----------

#[test]
fn item_8_brpg_strict_accepts_bit_1_page() {
    let key = [5u8; 32];
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w")), false),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.checkpoint().unwrap();
    }
    let s2 = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring(Some(BaceAuthentication::new(key, "w")), true),
    ).unwrap();
    assert_eq!(s2.get(b"k"), Some(b"v".to_vec()));
}

// ---------- §3 item 9 — non-strict preserves all tolerance ----------

#[test]
fn item_9_non_strict_wiring_tolerates_legacy_frames() {
    // Write under non-auth, reopen under auth-but-not-strict:
    // legacy opcode-4 + bit-0 frames remain readable.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring(None, false),
        ).unwrap();
        s.commit_batch(None, &large_batch(40, 24)).unwrap();
        s.checkpoint().unwrap();
    }
    let s2 = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(),
        wiring(Some(BaceAuthentication::new([1u8; 32], "k")), false),
    ).unwrap();
    assert_eq!(s2.scan().len(), 40);
}
