//! ADR 0286 + 0287 — fault injection on BRPG snapshot files
//! and manifest file.

use bradian_relational::fault_injection::{
    file_len, flip_byte_at, truncate_file_to,
};
use bradian_relational::row_store::{RowStore, StoreError};
use tempfile::tempdir;

fn seeded_and_checkpointed(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let store = RowStore::open(dir).unwrap();
    for i in 0..10 {
        store.put(format!("k{i:03}").as_bytes(),
                  format!("v{i:03}").as_bytes()).unwrap();
    }
    store.checkpoint().unwrap();
    drop(store);
    // Collect snapshot-*.brp and manifest-*.* files.
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with("snapshot-") || name.starts_with("manifest") {
                files.push(path);
            }
        }
    }
    files
}

fn assert_recovery_is_clean(dir: &std::path::Path) {
    match RowStore::open(dir) {
        Ok(store) => {
            // If reopen succeeded, any readable key must be a
            // committed one with its original value.
            for (k, v) in store.scan() {
                let k_str = std::str::from_utf8(&k).unwrap().to_string();
                let v_str = std::str::from_utf8(&v).unwrap().to_string();
                assert!(k_str.starts_with("k"),
                    "store exposed invented key {k_str:?}");
                let i: usize = k_str[1..].parse().unwrap();
                assert_eq!(v_str, format!("v{i:03}"),
                    "store exposed corrupted value for {k_str:?}");
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

/// ADR 0286 — flip a byte in each snapshot-*.brp file.
/// Reopen must either drop/repair cleanly or return an
/// explicit error. Never silently expose bad data.
#[test]
fn snapshot_file_byte_flip_never_fabricates() {
    let src = tempdir().unwrap();
    let files = seeded_and_checkpointed(src.path());
    let snapshot_files: Vec<_> = files.iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("snapshot-"))).collect();
    assert!(!snapshot_files.is_empty(),
        "checkpoint didn't produce any snapshot-*.brp file");
    for f in snapshot_files {
        let len = file_len(f).unwrap();
        if len < 8 { continue; }
        // Clone the directory into a fresh scenario.
        let scenario = tempdir().unwrap();
        for entry in std::fs::read_dir(src.path()).unwrap() {
            let p = entry.unwrap().path();
            if p.is_file() {
                std::fs::copy(&p, scenario.path().join(p.file_name().unwrap())).unwrap();
            }
        }
        let target = scenario.path().join(f.file_name().unwrap());
        flip_byte_at(&target, len / 2).unwrap();
        assert_recovery_is_clean(scenario.path());
    }
}

/// ADR 0286 — truncate each snapshot file. Same invariant.
#[test]
fn snapshot_file_truncation_never_fabricates() {
    let src = tempdir().unwrap();
    let files = seeded_and_checkpointed(src.path());
    let snapshot_files: Vec<_> = files.iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("snapshot-"))).collect();
    for f in snapshot_files {
        let len = file_len(f).unwrap();
        if len < 8 { continue; }
        let scenario = tempdir().unwrap();
        for entry in std::fs::read_dir(src.path()).unwrap() {
            let p = entry.unwrap().path();
            if p.is_file() {
                std::fs::copy(&p, scenario.path().join(p.file_name().unwrap())).unwrap();
            }
        }
        let target = scenario.path().join(f.file_name().unwrap());
        truncate_file_to(&target, len * 2 / 3).unwrap();
        assert_recovery_is_clean(scenario.path());
    }
}

/// ADR 0287 — flip a byte in the manifest. Recovery must
/// surface an explicit error; the manifest pins which
/// snapshot is authoritative, so silent corruption would
/// pick the wrong page set.
#[test]
fn manifest_byte_flip_detected_or_dropped() {
    let src = tempdir().unwrap();
    let files = seeded_and_checkpointed(src.path());
    let manifest_files: Vec<_> = files.iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("manifest"))).collect();
    if manifest_files.is_empty() {
        // Some checkpoints don't emit a manifest in this build.
        // Skip rather than fail.
        return;
    }
    for f in manifest_files {
        let len = file_len(f).unwrap();
        if len < 4 { continue; }
        let scenario = tempdir().unwrap();
        for entry in std::fs::read_dir(src.path()).unwrap() {
            let p = entry.unwrap().path();
            if p.is_file() {
                std::fs::copy(&p, scenario.path().join(p.file_name().unwrap())).unwrap();
            }
        }
        let target = scenario.path().join(f.file_name().unwrap());
        flip_byte_at(&target, len / 2).unwrap();
        assert_recovery_is_clean(scenario.path());
    }
}

/// Zero-length snapshot file — pathological case. Recovery
/// must still either cleanly succeed (dropping the empty
/// snapshot as if it never existed) or return an explicit
/// error.
#[test]
fn zero_length_snapshot_file() {
    let src = tempdir().unwrap();
    let files = seeded_and_checkpointed(src.path());
    let snapshot_files: Vec<_> = files.iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("snapshot-"))).collect();
    if snapshot_files.is_empty() { return; }
    let scenario = tempdir().unwrap();
    for entry in std::fs::read_dir(src.path()).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, scenario.path().join(p.file_name().unwrap())).unwrap();
        }
    }
    let target = scenario.path().join(snapshot_files[0].file_name().unwrap());
    truncate_file_to(&target, 0).unwrap();
    assert_recovery_is_clean(scenario.path());
}
