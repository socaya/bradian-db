//! ADR 0091 — strict authenticated mode: reject legacy v1
//! backup descriptors when auth wiring is strict.

use std::sync::Arc;

use bradian_relational::backup::{read_receipt_with_strict_auth, BackupReceipt};
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

// ---------- §3.3 item 1 — tolerance when strict=false ----------

#[test]
fn item_1_non_strict_wiring_still_accepts_v1_descriptor() {
    // Create a v1 (non-auth) backup, then read it with an auth
    // wiring that has strict=false. Must still work.
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let auth = BaceAuthentication::new([3u8; 32], "x");
    let r = read_receipt_with_strict_auth(
        &bak.join("backup.info"), Some(&auth), false,
    ).unwrap();
    assert_eq!(r.rows, 1);
}

// ---------- §3.3 item 2 — strict mode rejects v1 ----------

#[test]
fn item_2_strict_mode_rejects_v1_descriptor_under_auth() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let auth = BaceAuthentication::new([3u8; 32], "x");
    let err = read_receipt_with_strict_auth(
        &bak.join("backup.info"), Some(&auth), true,
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("strict mode"),
        "expected strict-mode rejection; got: {msg}");
}

// ---------- §3.3 item 3 — strict mode accepts v2 under matching key ----------

#[test]
fn item_3_strict_mode_accepts_v2_with_matching_key() {
    let key = [7u8; 32];
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open_with_bace(
            &db, StorageLimits::default(),
            wiring(Some(BaceAuthentication::new(key, "w")), false),
        ).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    let auth = BaceAuthentication::new(key, "w");
    let r = read_receipt_with_strict_auth(
        &bak.join("backup.info"), Some(&auth), true,
    ).unwrap();
    assert_eq!(r.rows, 1);
}

// ---------- §3.3 item 5 — strict without auth has no effect ----------

#[test]
fn item_5_strict_without_auth_has_no_effect() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    // strict=true but auth=None: strict is vacuous, read succeeds.
    let r = read_receipt_with_strict_auth(
        &bak.join("backup.info"), None, true,
    ).unwrap();
    assert_eq!(r.rows, 1);
}

// ---------- §3.3 item 2 — strict mode rejects at restore_backup call ----------

#[test]
fn item_2b_strict_mode_restore_backup_rejects_v1() {
    let dir = tempdir().unwrap();
    let db = dir.path().join("src");
    let bak = dir.path().join("bak");
    let restored = dir.path().join("restored");
    {
        let s = RowStore::open(&db).unwrap();
        s.put(b"k", b"v").unwrap();
        s.backup_to(&bak).unwrap();
    }
    // Use restore_backup_with_bace with a strict-auth wiring.
    let w = wiring(Some(BaceAuthentication::new([1u8; 32], "r")), true);
    let err = RowStore::restore_backup_with_bace(
        &bak, &restored, StorageLimits::default(), w,
    ).unwrap_err();
    let msg = format!("{err:?}");
    assert!(msg.contains("strict mode"),
        "expected strict-mode rejection at restore; got: {msg}");
    assert!(!restored.exists(), "no destination dir must be created on strict reject");
}

// ---------- §3.3 item 6 — decode_with_strict_auth direct contract ----------

#[test]
fn item_6_decode_with_strict_auth_direct() {
    let r = BackupReceipt {
        checkpoint_lsn: 3, last_lsn: 7, rows: 5,
        flags: Default::default(),
    };
    let bytes = r.encode();
    // strict=false, no auth → ok.
    assert!(BackupReceipt::decode_with_strict_auth(&bytes, None, false).is_ok());
    // strict=true, no auth → still ok (strict is vacuous).
    assert!(BackupReceipt::decode_with_strict_auth(&bytes, None, true).is_ok());
    let auth = BaceAuthentication::new([3u8; 32], "x");
    // strict=false, auth=Some → ok (tolerance).
    assert!(BackupReceipt::decode_with_strict_auth(&bytes, Some(&auth), false).is_ok());
    // strict=true, auth=Some → rejected.
    let err = BackupReceipt::decode_with_strict_auth(&bytes, Some(&auth), true).unwrap_err();
    assert!(format!("{err:?}").contains("strict mode"));
}
