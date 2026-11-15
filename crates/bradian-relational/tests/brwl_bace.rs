//! ADR 0077 — BRWL carrier BACE adapter.
//!
//! §3.6 compliance tests covering the writer/reader opcode dispatch,
//! legacy compatibility, and threshold behavior. §3.5 benchmarks
//! live in a sibling file (`brwl_bace_benchmark.rs`) so the pass/fail
//! threshold checks there drive the ADR acceptance decision.

use std::sync::Arc;

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::codec::zstd::ZSTD_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::{
    BaceWiring, RowMutation, RowStore, StoreError, BRWL_BACE_MIN_BODY_BYTES,
};
use bradian_relational::runtime_limits::StorageLimits;
use tempfile::tempdir;

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
        previous_authentication: None,
    }
}

/// Build mutations whose combined payload-after-opcode exceeds the
/// §3.2 threshold. Each entry is ~20 bytes record overhead + key + value.
fn large_batch(count: usize, value_bytes: usize) -> Vec<RowMutation> {
    (0..count).map(|i| RowMutation::Put {
        key: format!("k{:08}", i).into_bytes(),
        value: vec![b'x'; value_bytes],
    }).collect()
}

fn snapshot(dir: &std::path::Path, bace: Option<BaceWiring>) -> Vec<(Vec<u8>, Vec<u8>)> {
    let store = match bace {
        Some(w) => RowStore::open_with_bace(dir, StorageLimits::default(), w).unwrap(),
        None => RowStore::open(dir).unwrap(),
    };
    store.scan()
}

#[test]
fn item_1_single_op_commit_under_bace_wiring_stays_v1_below_threshold() {
    // ADR 0077 §3.6 item 1 — small single-op put under BACE wiring
    // produces a v1 frame (opcode 1 or 3 after single-op lift to
    // commit_batch), not opcode 4.
    //
    // The RowStore::put path encodes through `encode_wal` which is
    // always v1 single-op; the opcode-4 path is reachable only via
    // commit_batch. A tiny commit_batch also stays v1 (below
    // threshold). Both are covered.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.put(b"tiny", b"v").unwrap();
        s.commit_batch(None, &[
            RowMutation::Put { key: b"k".to_vec(), value: b"v".to_vec() },
        ]).unwrap();
    }

    // Inspect the WAL file directly. First two frames should be v1
    // (opcode 1 for the single put, opcode 3 for the small batch).
    let wal = std::fs::read(dir.path().join("rows.wal")).unwrap();
    // WAL_HEADER = 12 bytes; body[0..8] = LSN; body[8] = opcode.
    assert!(wal.len() > 24);
    let first_op = wal[12 + 8];
    assert!(first_op == 1 || first_op == 3,
        "first frame opcode {first_op} should be 1 or 3 (v1), not 4");

    // Second frame starts after first frame = 12 header + len bytes.
    let first_len = u32::from_le_bytes(wal[4..8].try_into().unwrap()) as usize;
    let second_start = 12 + first_len;
    let second_body_op = wal[second_start + 12 + 8];
    assert!(second_body_op == 1 || second_body_op == 3,
        "second frame opcode {second_body_op} should be 1 or 3 (v1), not 4");
}

#[test]
fn item_2_large_batch_under_bace_wiring_emits_v2_opcode_4() {
    // ADR 0077 §3.6 item 2 — large batch body under BACE wiring
    // emits opcode 4. Threshold is 512 bytes; a 32-op batch with
    // 32-byte values easily exceeds that.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.commit_batch(None, &large_batch(32, 64)).unwrap();
    }

    let wal = std::fs::read(dir.path().join("rows.wal")).unwrap();
    let opcode = wal[12 + 8];
    assert_eq!(opcode, 4, "large batch under BACE wiring must emit opcode 4");
}

#[test]
fn item_3_v2_frame_round_trips_through_replay() {
    // ADR 0077 §3.6 item 3 — v2 WAL replay yields the same rows.
    let dir = tempdir().unwrap();
    let mutations = large_batch(48, 32);
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.commit_batch(None, &mutations).unwrap();
    }

    let rows = snapshot(dir.path(), Some(wiring_zstd_balanced()));
    assert_eq!(rows.len(), 48);
    for (i, (k, v)) in rows.iter().enumerate() {
        assert_eq!(k, &format!("k{:08}", i).into_bytes());
        assert_eq!(v.len(), 32);
    }
}

#[test]
fn item_4_legacy_open_on_v2_wal_fails_closed() {
    // ADR 0077 §3.6 item 4 — legacy RowStore::open on a WAL with
    // opcode-4 frames fails closed.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.commit_batch(None, &large_batch(32, 64)).unwrap();
    }

    match RowStore::open(dir.path()) {
        Err(StoreError::Corruption(msg)) => {
            assert!(
                msg.contains("BACE") || msg.contains("open_with_bace"),
                "expected BACE-origin corruption message, got: {msg}",
            );
        }
        other => panic!("expected Corruption, got {other:?}"),
    }
}

#[test]
fn item_5_open_with_bace_on_pre_adr_0077_wal_succeeds() {
    // ADR 0077 §3.6 item 5 — a WAL with only opcodes 1/2/3 (pre-
    // ADR-0077 writers) must reopen under _with_bace unchanged.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open(dir.path()).unwrap();
        s.put(b"a", b"1").unwrap();
        s.commit_batch(None, &[
            RowMutation::Put { key: b"b".to_vec(), value: b"2".to_vec() },
            RowMutation::Delete { key: b"a".to_vec() },
        ]).unwrap();
    }

    let s = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
    ).unwrap();
    assert_eq!(s.get(b"a"), None);
    assert_eq!(s.get(b"b"), Some(b"2".to_vec()));
}

#[test]
fn item_6_tampered_v2_envelope_fails_closed() {
    // ADR 0077 §3.6 item 6 — bit-flip inside the BACE envelope of a
    // v2 frame fails closed. The outer WAL CRC covers the envelope
    // bytes AND the envelope has its own internal CRC, so either
    // layer detects the tamper.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
        ).unwrap();
        s.commit_batch(None, &large_batch(32, 64)).unwrap();
    }
    let wal_path = dir.path().join("rows.wal");
    let mut bytes = std::fs::read(&wal_path).unwrap();
    // Flip a byte deep inside the envelope (past the WAL header
    // and well into the payload).
    let flip = 12 + 9 + 40;   // WAL_HEADER + LSN+op + 40 bytes into envelope
    bytes[flip] ^= 0xFF;
    std::fs::write(&wal_path, bytes).unwrap();

    let err = RowStore::open_with_bace(
        dir.path(), StorageLimits::default(), wiring_zstd_balanced(),
    ).unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("CRC") || msg.contains("crc") || msg.contains("envelope")
            || msg.contains("BACE") || msg.contains("corruption"),
        "expected CRC/envelope/corruption error, got: {msg}",
    );
}

#[test]
fn item_7_threshold_constant_is_publicly_visible() {
    // ADR 0077 §3.6 item 7 — the threshold is a pub const operators
    // can observe. Default documented as 512 per §3.2.
    assert_eq!(BRWL_BACE_MIN_BODY_BYTES, 512);
}
