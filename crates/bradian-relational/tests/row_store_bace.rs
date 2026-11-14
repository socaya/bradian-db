//! `RowStore` BACE integration tests (accepted ADR 0065).
//!
//! Tests land sub-slice by sub-slice:
//! - R.1 (this commit): scaffolding only — `open_with_bace` exists,
//!   `bace_wiring` field stored, no behavior change yet. No v2 pages
//!   written or read. Checkpoint + recovery still take the v1 path.
//! - R.2 — R.4: expand this suite with the full 12-item checklist
//!   from ADR 0065 §4.

use std::sync::Arc;

use bradian_relational::bace::codec::raw::RAW_ID;
use bradian_relational::bace::policy::{
    CompressionProfile, InMemoryPolicyService, PlatformBaseline, PolicyKey, PolicyService,
    PolicyValue, ScopeChain,
};
use bradian_relational::bace::registry::BuiltInRegistry;
use bradian_relational::row_store::{BaceWiring, RowStore};
use bradian_relational::runtime_limits::StorageLimits;
use tempfile::tempdir;

#[allow(dead_code)]
fn wiring_default() -> BaceWiring {
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(
            PlatformBaseline::shipped(),
            BuiltInRegistry::new(),
        )),
        registry: Arc::new(BuiltInRegistry::new()),
    }
}

#[test]
fn open_with_bace_opens_fresh_directory_without_error() {
    let dir = tempdir().unwrap();
    let store = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_with_profile(CompressionProfile::Raw)).unwrap();
    assert_eq!(store.last_lsn(), 0);
}

#[test]
fn open_with_bace_round_trips_plain_writes_without_touching_disk_format() {
    // R.1 scaffolding: the wiring is stored but neither checkpoint
    // nor recovery consult it yet. Round-trip through write + reopen
    // must look exactly like a non-BACE round-trip at the row level.
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_with_profile(CompressionProfile::Raw)).unwrap();
        store.put(b"alpha", b"one").unwrap();
        store.put(b"beta", b"two").unwrap();
        store.checkpoint().unwrap();
    }
    let reopened = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_with_profile(CompressionProfile::Raw)).unwrap();
    assert_eq!(reopened.get(b"alpha"), Some(b"one".to_vec()));
    assert_eq!(reopened.get(b"beta"), Some(b"two".to_vec()));
}

#[test]
fn legacy_open_still_round_trips_against_an_open_with_bace_store() {
    // R.1: because open_with_bace does not yet emit v2 pages (writer
    // branch lands in R.2), a store written through open_with_bace
    // produces v1-shape snapshots that a legacy open can still read.
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_with_profile(CompressionProfile::Raw)).unwrap();
        store.put(b"k", b"v").unwrap();
        store.checkpoint().unwrap();
    }
    let legacy = RowStore::open(dir.path()).unwrap();
    assert_eq!(legacy.get(b"k"), Some(b"v".to_vec()));
}

#[test]
fn bace_wiring_can_be_constructed_with_a_raw_profile_policy() {
    // Smoke test that the policy types from bace::policy compose
    // with BaceWiring without any manual wiring work.
    let mut scope = ScopeChain::baseline_only();
    scope.job.insert(
        PolicyKey::CompressionProfile,
        PolicyValue::Profile(CompressionProfile::Raw),
    );
    scope.job.insert(
        PolicyKey::CompressionAllowedCodecs,
        PolicyValue::CodecSet(vec![RAW_ID]),
    );
    let svc = InMemoryPolicyService::new(PlatformBaseline::shipped(), BuiltInRegistry::new());
    let profile = svc.resolve(PolicyKey::CompressionProfile, &scope);
    assert_eq!(profile.value.as_profile(), Some(CompressionProfile::Raw));
}

// ---------- R.2: checkpoint writer emits v2 under non-Raw profile ----------

use bradian_relational::bace::codec::CodecId;
use bradian_relational::bace::envelope::ENVELOPE_MAGIC;
use bradian_relational::row_store::StoreError;
use std::fs;

fn wiring_with_profile(profile: CompressionProfile) -> BaceWiring {
    // Construct an InMemoryPolicyService whose baseline profile is
    // the given value, then apply no further overrides. The test
    // scope chain is baseline-only, so this is what resolves.
    let baseline = PlatformBaseline {
        profile,
        ..PlatformBaseline::shipped()
    };
    BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(
            baseline,
            BuiltInRegistry::new(),
        )),
        registry: Arc::new(BuiltInRegistry::new()),
    }
}

fn read_page_slice<const N: usize>(root: &std::path::Path, lsn: u64, page_offset: usize) -> [u8; N] {
    let bytes = fs::read(root.join(format!("snapshot-{lsn}.brp"))).unwrap();
    let start = page_offset * 4096;
    bytes[start..start + N].try_into().unwrap()
}

#[test]
fn raw_profile_emits_v1_bit_identical_to_legacy_open() {
    // Under CompressionProfile::Raw, the checkpoint writer must stay
    // on the v1 code path. The resulting bytes must be byte-identical
    // to a checkpoint written through the legacy open path on the
    // same rows.
    let legacy_dir = tempdir().unwrap();
    {
        let s = RowStore::open(legacy_dir.path()).unwrap();
        s.put(b"a", b"1").unwrap();
        s.put(b"b", b"2").unwrap();
        s.checkpoint().unwrap();
    }
    let bace_dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            bace_dir.path(), StorageLimits::default(),
            wiring_with_profile(CompressionProfile::Raw),
        ).unwrap();
        s.put(b"a", b"1").unwrap();
        s.put(b"b", b"2").unwrap();
        s.checkpoint().unwrap();
    }
    let legacy_bytes = fs::read(legacy_dir.path().join("snapshot-2.brp")).unwrap();
    let bace_bytes = fs::read(bace_dir.path().join("snapshot-2.brp")).unwrap();
    assert_eq!(legacy_bytes, bace_bytes,
        "Raw profile must emit byte-identical v1 snapshots");
}

#[test]
fn balanced_profile_emits_v2_data_pages_with_bace_envelope_flag_set() {
    // Under CompressionProfile::Balanced, the checkpoint writer must
    // emit v2 data pages whose payload begins with the BACE envelope
    // magic. The metadata page (page 0) stays v1 per ADR 0065 §6.7.
    let dir = tempdir().unwrap();
    {
        let s = RowStore::open_with_bace(
            dir.path(), StorageLimits::default(),
            wiring_with_profile(CompressionProfile::Balanced),
        ).unwrap();
        s.put(b"alpha", b"one").unwrap();
        s.put(b"beta", b"two").unwrap();
        s.checkpoint().unwrap();
    }

    // Metadata page 0: v1 (format_version = 1, byte [7] = 0).
    let meta_header: [u8; 8] = read_page_slice(dir.path(), 2, 0);
    assert_eq!(&meta_header[0..4], b"BRPG");
    assert_eq!(u16::from_le_bytes([meta_header[4], meta_header[5]]), 1);
    assert_eq!(meta_header[7], 0);

    // Data page 1: v2 with payload_is_bace_envelope bit set.
    let data_header: [u8; 8] = read_page_slice(dir.path(), 2, 1);
    assert_eq!(&data_header[0..4], b"BRPG");
    assert_eq!(u16::from_le_bytes([data_header[4], data_header[5]]), 2,
        "data page must be BRPG v2");
    assert_eq!(data_header[7] & 0b0000_0001, 0b0000_0001,
        "payload_is_bace_envelope flag must be set");

    // Data page payload starts at offset 32 of page 1 and begins with
    // ENVELOPE_MAGIC (BACE envelope header).
    let bytes = fs::read(dir.path().join("snapshot-2.brp")).unwrap();
    let payload_start = 4096 + 32;
    assert_eq!(&bytes[payload_start..payload_start + 4], &ENVELOPE_MAGIC);
}

#[test]
fn checkpoint_fails_closed_when_allowed_codec_set_contains_only_unregistered_ids() {
    // A policy whose allow-set names only unregistered codec IDs must
    // fail the checkpoint with BaceCodecUnavailable. The prior
    // committed state stays intact: no .brp is published, no
    // manifest update, no WAL truncation.
    let dir = tempdir().unwrap();

    // Build a wiring whose baseline allow-set is a single unregistered
    // codec ID (16 — inner-encoding range, unassigned per ADR 0061 P1
    // deferred).
    let baseline = PlatformBaseline {
        profile: CompressionProfile::Balanced,
        allowed_codecs: vec![CodecId(16)],
        ..PlatformBaseline::shipped()
    };
    let wiring = BaceWiring {
        policy_service: Arc::new(InMemoryPolicyService::new(
            baseline,
            BuiltInRegistry::new(),
        )),
        registry: Arc::new(BuiltInRegistry::new()),
    };
    let s = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring).unwrap();
    s.put(b"k", b"v").unwrap();
    // The policy service's capability veto will fall back to the
    // platform baseline when the user-set allow-set is fully
    // unregistered — but the platform baseline from PlatformBaseline
    // itself contains only CodecId(16) here, so the veto's fallback
    // sees the same unregistered set and returns it unchanged.
    // The writer then fails closed because no codec in the final
    // allow-set is registered.
    let err = s.checkpoint().unwrap_err();
    assert!(matches!(err, StoreError::BaceCodecUnavailable { codec_id: CodecId(16) }),
        "expected BaceCodecUnavailable, got {err:?}");
    // Prior state intact: no snapshot file, no manifest.
    assert!(!dir.path().join("snapshot-1.brp").exists());
    assert!(!dir.path().join("checkpoint.current").exists());
}
