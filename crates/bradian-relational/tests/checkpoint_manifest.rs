//! Real-file recovery tests for the atomic Bradian checkpoint generation marker.

use std::fs;
use bradian_relational::checkpoint_manifest::{self, CheckpointManifest, ManifestError};
use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

#[test]
fn binary_manifest_roundtrip_and_corruption_detection() {
    let meta = CheckpointManifest { lsn: 42, rows: 800, data_pages: 32, index_pages: 10 };
    let bytes = meta.encode();
    assert_eq!(bytes.len(), 40);
    assert_eq!(&bytes[0..4], b"BRCP");
    assert_eq!(CheckpointManifest::decode(&bytes).unwrap(), meta);
    for at in [0, 4, 7, 9, 19, 27, 35, 39] {
        let mut damaged = bytes;
        damaged[at] ^= 0x51;
        assert!(CheckpointManifest::decode(&damaged).is_err(), "byte {at}");
    }
    assert!(CheckpointManifest::decode(&bytes[..39]).is_err());
}

#[test]
fn checkpoint_publishes_manifest_and_restarts_using_it() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        for i in 0..150 {
            store.put(format!("patient-{i:04}").as_bytes(), b"registered").unwrap();
        }
        assert_eq!(store.checkpoint().unwrap(), 150);
        let manifest = checkpoint_manifest::load(dir.path()).unwrap().unwrap();
        assert_eq!(manifest.lsn, 150);
        assert_eq!(manifest.rows, 150);
        assert!(manifest.data_pages > 0);
        assert!(manifest.index_pages > 0);
        checkpoint_manifest::check_snapshot(dir.path(), manifest).unwrap();
        checkpoint_manifest::check_index(dir.path(), manifest).unwrap();
    }
    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.last_lsn(), 150);
    assert_eq!(reopened.len(), 150);
    assert_eq!(reopened.indexed_get(b"patient-0001").unwrap(), Some(b"registered".to_vec()));
}

#[test]
fn manifest_prevents_orphan_higher_snapshot_from_becoming_authoritative() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"one", b"committed").unwrap();
        store.checkpoint().unwrap(); // authoritative manifest = 1
        store.put(b"two", b"only-in-WAL").unwrap();
    }
    let authoritative = fs::read(dir.path().join("snapshot-1.brp")).unwrap();
    let mut orphan = authoritative.clone();
    // A newer snapshot filename is NOT a published generation.
    // The contents deliberately belong to an old checkpoint and must not
    // be selected just because this file sorts later.
    fs::write(dir.path().join("snapshot-2.brp"), &orphan).unwrap();
    fs::write(dir.path().join("index-2.bri"), b"interrupted").unwrap();
    let reopened = RowStore::open(dir.path()).unwrap();
    assert_eq!(reopened.get(b"one"), Some(b"committed".to_vec()));
    assert_eq!(reopened.get(b"two"), Some(b"only-in-WAL".to_vec()));
    assert_eq!(reopened.last_lsn(), 2);
    drop(reopened);
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(store.checkpoint().unwrap(), 2);
    assert!(dir.path().join("snapshot-2.brp.orphan").exists());
    assert!(dir.path().join("index-2.bri.orphan").exists());
    assert_eq!(checkpoint_manifest::load(dir.path()).unwrap().unwrap().lsn, 2);
    assert_eq!(store.indexed_get(b"two").unwrap(), Some(b"only-in-WAL".to_vec()));
    orphan.clear();
}

#[test]
fn corrupted_existing_manifest_cannot_fall_back_to_legacy_snapshot_selection() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"id", b"safe").unwrap();
        store.checkpoint().unwrap();
    }
    let path = dir.path().join("checkpoint.current");
    let mut bytes = fs::read(&path).unwrap();
    bytes[16] ^= 0x6f;
    fs::write(&path, bytes).unwrap();
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Manifest(_))));
}

#[test]
fn missing_or_mismatched_manifest_snapshot_is_an_integrity_failure() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"row", b"value").unwrap();
        store.checkpoint().unwrap();
    }
    fs::remove_file(dir.path().join("snapshot-1.brp")).unwrap();
    assert!(matches!(RowStore::open(dir.path()), Err(StoreError::Manifest(_))));
}

#[test]
fn interrupted_manifest_pending_file_never_replaces_current() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"base", b"true").unwrap();
        store.checkpoint().unwrap();
        store.put(b"new", b"wal").unwrap();
    }
    fs::write(dir.path().join("checkpoint.pending"), b"torn").unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(checkpoint_manifest::load(dir.path()).unwrap().unwrap().lsn, 1);
    assert_eq!(store.get(b"new"), Some(b"wal".to_vec()));
    assert_eq!(store.checkpoint().unwrap(), 2);
    assert_eq!(checkpoint_manifest::load(dir.path()).unwrap().unwrap().lsn, 2);
}

#[test]
fn pre_manifest_legacy_checkpoint_is_migrated_without_losing_rows() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"first", b"historical").unwrap();
        store.checkpoint().unwrap();
        store.put(b"second", b"new").unwrap();
    }
    fs::remove_file(dir.path().join("checkpoint.current")).unwrap(); // emulate old format
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(store.get(b"first"), Some(b"historical".to_vec()));
    assert_eq!(store.get(b"second"), Some(b"new".to_vec()));
    assert_eq!(store.checkpoint().unwrap(), 2);
    assert_eq!(checkpoint_manifest::load(dir.path()).unwrap().unwrap().lsn, 2);
}

#[test]
fn manifest_never_regresses_checkpoint_lsn() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"one", b"data").unwrap();
        store.checkpoint().unwrap();
    }
    let same = checkpoint_manifest::load(dir.path()).unwrap().unwrap();
    assert!(matches!(checkpoint_manifest::publish(dir.path(), same), Err(ManifestError::Regressed)));
    assert_eq!(checkpoint_manifest::load(dir.path()).unwrap().unwrap(), same);
}

#[test]
fn missing_index_with_committed_manifest_can_be_regenerated() {
    let dir = tempdir().unwrap();
    {
        let store = RowStore::open(dir.path()).unwrap();
        store.put(b"record", b"present").unwrap();
        store.checkpoint().unwrap();
    }
    fs::remove_file(dir.path().join("index-1.bri")).unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    assert_eq!(store.indexed_get(b"record").unwrap(), Some(b"present".to_vec()));
    assert_eq!(checkpoint_manifest::load(dir.path()).unwrap().unwrap().lsn, 1);
}
