//! ADR 0290 — fsync-on-power-loss simulation.
//!
//! A real power-loss rig observes "the fsync returned Ok
//! but a subsequent crash lost post-fsync bytes." We
//! simulate this at the file level: capture the WAL
//! length right after a fsync-equivalent point (checkpoint
//! or clean close), continue writing, then truncate back
//! to the captured length. That IS the file state a lying
//! fsync would leave on disk after a power cut.
//!
//! The reopen must then show only pre-sync committed
//! state, with no fabricated or corrupted trailing data.

use bradian_relational::fault_injection::{file_len, truncate_file_to};
use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn simulated_power_loss_rolls_back_to_last_synced_state() {
    let dir = tempdir().unwrap();
    // Phase 1: write 5 rows and checkpoint — this is the
    // "last durable state" that a real fsync-on-power-loss
    // rig would preserve.
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..5 {
        store.put(format!("k{i:03}").as_bytes(),
                  format!("v{i:03}").as_bytes()).unwrap();
    }
    store.checkpoint().unwrap();
    drop(store);
    let wal = dir.path().join("rows.wal");
    let sync_point_len = file_len(&wal).unwrap();

    // Phase 2: write 5 MORE rows WITHOUT another checkpoint.
    let store = RowStore::open(dir.path()).unwrap();
    for i in 5..10 {
        store.put(format!("k{i:03}").as_bytes(),
                  format!("v{i:03}").as_bytes()).unwrap();
    }
    drop(store);
    let after_more_len = file_len(&wal).unwrap();
    assert!(after_more_len > sync_point_len,
        "WAL did not grow after the second write phase");

    // Phase 3: simulate a lying-fsync power loss by
    // truncating the WAL back to the sync point. This is
    // what the disk would retain after the power cut.
    truncate_file_to(&wal, sync_point_len).unwrap();

    // Phase 4: reopen. The store must either expose only
    // the first 5 keys, or return an explicit error. Never
    // silent partial data.
    match RowStore::open(dir.path()) {
        Ok(store) => {
            for i in 0..5 {
                let key = format!("k{i:03}").into_bytes();
                let val = format!("v{i:03}").into_bytes();
                assert_eq!(store.get(&key).as_deref(), Some(val.as_slice()),
                    "pre-sync committed key {i} missing after simulated power loss");
            }
            // Keys 5..10 may or may not be visible depending
            // on where the truncation landed vs. frame
            // boundaries. If they ARE visible, the value must
            // be the one we wrote (no corruption).
            for i in 5..10 {
                let key = format!("k{i:03}").into_bytes();
                let want = format!("v{i:03}").into_bytes();
                if let Some(v) = store.get(&key) {
                    assert_eq!(v.as_slice(), want.as_slice(),
                        "post-sync key {i} exposed corrupted value");
                }
            }
        }
        Err(err) => assert!(
            matches!(err, StoreError::Corruption(_) | StoreError::Io(_)
                | StoreError::InvalidRow(_) | StoreError::Page(_)
                | StoreError::Manifest(_)),
            "expected explicit recovery error after power-loss sim, got {err:?}",
        ),
    }
}

#[test]
fn simulated_power_loss_mid_frame_never_fabricates() {
    // Even if the truncation lands mid-frame (between frame
    // header and body), reopen must not expose a
    // fabricated row.
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..5 {
        store.put(format!("k{i:03}").as_bytes(),
                  format!("v{i:03}").as_bytes()).unwrap();
    }
    store.checkpoint().unwrap();
    drop(store);
    let wal = dir.path().join("rows.wal");
    let synced_len = file_len(&wal).unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 5..10 {
        store.put(format!("k{i:03}").as_bytes(),
                  format!("v{i:03}").as_bytes()).unwrap();
    }
    drop(store);
    let full_len = file_len(&wal).unwrap();
    // Truncate to the midpoint between the synced boundary
    // and the final state — likely mid-frame.
    let mid = synced_len + (full_len - synced_len) / 2;
    truncate_file_to(&wal, mid).unwrap();
    match RowStore::open(dir.path()) {
        Ok(store) => {
            // No invented key may appear.
            for (k, _v) in store.scan() {
                let k_str = std::str::from_utf8(&k).unwrap();
                assert!(k_str.starts_with("k"),
                    "simulated power loss exposed invented key {k_str:?}");
                let idx: usize = k_str[1..].parse().unwrap();
                assert!(idx < 10,
                    "simulated power loss exposed out-of-range key index {idx}");
            }
            // The first 5 keys (pre-sync) must still be there.
            for i in 0..5 {
                let k = format!("k{i:03}").into_bytes();
                assert!(store.get(&k).is_some(),
                    "pre-sync key {i} lost after mid-frame power loss");
            }
        }
        Err(err) => assert!(
            matches!(err, StoreError::Corruption(_) | StoreError::Io(_)
                | StoreError::InvalidRow(_) | StoreError::Page(_)
                | StoreError::Manifest(_)),
            "expected explicit recovery error, got {err:?}",
        ),
    }
}

#[test]
fn simulated_power_loss_before_first_checkpoint() {
    // No checkpoint happened — the WAL was the only
    // durable log. Power-loss simulated as full truncation.
    let dir = tempdir().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..5 {
        store.put(format!("k{i:03}").as_bytes(),
                  format!("v{i:03}").as_bytes()).unwrap();
    }
    drop(store);
    let wal = dir.path().join("rows.wal");
    // Truncate to zero: no durable state at all.
    truncate_file_to(&wal, 0).unwrap();
    match RowStore::open(dir.path()) {
        Ok(store) => {
            // No invented data.
            assert!(store.scan().is_empty(),
                "simulated full power loss exposed phantom rows");
        }
        Err(err) => assert!(
            matches!(err, StoreError::Corruption(_) | StoreError::Io(_)
                | StoreError::InvalidRow(_) | StoreError::Page(_)
                | StoreError::Manifest(_)),
            "expected explicit recovery error, got {err:?}",
        ),
    }
}
