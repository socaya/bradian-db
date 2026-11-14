//! Bounded admission for directory listings (ADR 0051).
//!
//! The pre-manifest snapshot listing and legacy migration enumerate inputs that
//! are not bounded by anything else in the storage protocol. These tests cover:
//!   - entry-count admission on the row-store recovery path
//!   - name-byte admission on the same path
//!   - admission preserves on-disk state (quota is not corruption)
//!   - default limits accept normal workloads
//!   - legacy migration carries directory limits through its new `_with_policies`

use std::fs;

use bradian_relational::row_store::{RowStore, StoreError};
use bradian_relational::runtime_limits::{
    DirectoryLimits, IndexLimits, IndexBuildLimits, ResourceError, RowStoreLimits, StorageLimits,
};

fn policies(limits: DirectoryLimits) -> StorageLimits {
    StorageLimits {
        rows: RowStoreLimits::default(),
        index: IndexLimits::default(),
        build: IndexBuildLimits::default(),
        directory: limits,
    }
}

fn quota(err: StoreError, resource: &str) {
    match err {
        StoreError::Resource(ResourceError::DirectoryLimit { resource: actual, .. }) => {
            assert_eq!(actual, resource);
        }
        other => panic!("expected DirectoryLimit({resource}), got {other:?}"),
    }
}

fn build_pre_manifest_store(root: &std::path::Path) -> u64 {
    let db = RowStore::open(root).unwrap();
    db.put(b"a", b"v").unwrap();
    let lsn = db.checkpoint().unwrap();
    drop(db);
    // Force the pre-manifest discovery path on reopen.
    fs::remove_file(root.join("checkpoint.current")).unwrap();
    lsn
}

#[test]
fn snapshot_listing_admits_entries_and_refuses_oversized_directory() {
    let dir = tempfile::tempdir().unwrap();
    let _lsn = build_pre_manifest_store(dir.path());

    // Count actual entries after checkpoint so limits can be tightened to exactly the
    // observed inventory. The set of files is a function of the storage protocol, not
    // the test, so this adapts if new files are added.
    let actual_entries = fs::read_dir(dir.path()).unwrap().count();
    assert!(actual_entries > 0);

    // Entry count refusal: one fewer than actual is enough to trip admission.
    let too_tight = DirectoryLimits { max_entries: actual_entries - 1, ..DirectoryLimits::default() };
    let err = RowStore::open_with_policies(dir.path(), policies(too_tight)).unwrap_err();
    quota(err, "entries");

    // Nothing was published or deleted by the refused open.
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), actual_entries);

    // Exact capacity admits the listing.
    let exact = DirectoryLimits { max_entries: actual_entries, ..DirectoryLimits::default() };
    let db = RowStore::open_with_policies(dir.path(), policies(exact)).unwrap();
    assert_eq!(db.get(b"a"), Some(b"v".to_vec()));
}

#[test]
fn snapshot_listing_admits_name_bytes_and_refuses_oversized_names() {
    let dir = tempfile::tempdir().unwrap();
    let _lsn = build_pre_manifest_store(dir.path());

    // A single-byte budget cannot fit any real on-disk filename; the first entry
    // trips admission, still reporting the directory-name-bytes resource.
    let too_tight = DirectoryLimits { max_name_bytes: 1, ..DirectoryLimits::default() };
    let err = RowStore::open_with_policies(dir.path(), policies(too_tight)).unwrap_err();
    quota(err, "name bytes");

    // A generous budget works and the pre-manifest store reopens.
    let roomy = DirectoryLimits { max_name_bytes: 4096, ..DirectoryLimits::default() };
    let db = RowStore::open_with_policies(dir.path(), policies(roomy)).unwrap();
    assert_eq!(db.get(b"a"), Some(b"v".to_vec()));
}

#[test]
fn default_directory_limits_admit_an_ordinary_store_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    {
        let db = RowStore::open(dir.path()).unwrap();
        db.put(b"a", b"v").unwrap();
        db.checkpoint().unwrap();
        db.put(b"b", b"v").unwrap();
    }
    // Default policies include the default directory budget; ordinary reopen succeeds.
    let db = RowStore::open(dir.path()).unwrap();
    assert_eq!(db.get(b"a"), Some(b"v".to_vec()));
    assert_eq!(db.get(b"b"), Some(b"v".to_vec()));
}

#[test]
fn directory_admission_refusal_preserves_storage_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let _lsn = build_pre_manifest_store(dir.path());

    // Snapshot the directory contents before the refused open.
    let before = sorted_entries(dir.path());

    // Entry admission runs before name admission in the helper, so a 1/1 budget
    // trips on the first entry's name bytes — the first entry itself is still admitted.
    let too_tight = DirectoryLimits { max_entries: 1, max_name_bytes: 1 };
    let err = RowStore::open_with_policies(dir.path(), policies(too_tight)).unwrap_err();
    quota(err, "name bytes");

    let after = sorted_entries(dir.path());
    assert_eq!(before, after, "directory contents must be unchanged by refused open");
}

fn sorted_entries(root: &std::path::Path) -> Vec<(String, u64)> {
    let mut entries: Vec<(String, u64)> = fs::read_dir(root)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            let name = e.file_name().into_string().unwrap();
            let len = e.metadata().unwrap().len();
            (name, len)
        })
        .collect();
    entries.sort();
    entries
}

#[test]
fn directory_limits_are_exposed_through_storage_policies_and_round_trip_through_restore() {
    // The post-open accessor reports the directory policy the store was opened with,
    // and the same policy flows into the restore validator. A manifested restore does
    // not exercise the pre-manifest listing, so the test asserts plumbing, not refusal.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("source");
    let configured = DirectoryLimits { max_entries: 512, max_name_bytes: 32 * 1024 };
    let db = RowStore::open_with_policies(&source, policies(configured)).unwrap();
    db.put(b"k", b"v").unwrap();
    db.checkpoint().unwrap();
    assert_eq!(db.storage_limits().directory.max_entries, configured.max_entries);
    assert_eq!(db.storage_limits().directory.max_name_bytes, configured.max_name_bytes);
    let archive = dir.path().join("archive");
    db.backup_current_generation(&archive).unwrap();
    drop(db);

    let restored_path = dir.path().join("restored");
    RowStore::restore_backup_with_policies(&archive, &restored_path, policies(configured)).unwrap();
    let restored = RowStore::open_with_policies(&restored_path, policies(configured)).unwrap();
    assert_eq!(restored.get(b"k"), Some(b"v".to_vec()));
    assert_eq!(restored.storage_limits().directory.max_entries, configured.max_entries);
    assert_eq!(restored.storage_limits().directory.max_name_bytes, configured.max_name_bytes);
}
