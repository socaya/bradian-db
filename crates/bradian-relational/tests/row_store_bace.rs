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
    let store = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_default()).unwrap();
    assert_eq!(store.last_lsn(), 0);
}

#[test]
fn open_with_bace_round_trips_plain_writes_without_touching_disk_format() {
    // R.1 scaffolding: the wiring is stored but neither checkpoint
    // nor recovery consult it yet. Round-trip through write + reopen
    // must look exactly like a non-BACE round-trip at the row level.
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_default()).unwrap();
        store.put(b"alpha", b"one").unwrap();
        store.put(b"beta", b"two").unwrap();
        store.checkpoint().unwrap();
    }
    let reopened = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_default()).unwrap();
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
        let store = RowStore::open_with_bace(dir.path(), StorageLimits::default(), wiring_default()).unwrap();
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
