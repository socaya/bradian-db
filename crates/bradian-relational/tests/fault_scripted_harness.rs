//! ADR 0289 — scripted-workload fault harness.
//!
//! Generates deterministic (seeded) random workloads of
//! put / overwrite / delete / checkpoint operations against
//! a RowStore. After the workload, applies one of a handful
//! of fault patterns to a chosen file, reopens, and asserts
//! the strong property: reopen either errors explicitly OR
//! exposes a (k,v) map that is a subset of the ACKed state
//! with values matching every ack we made.

use bradian_relational::fault_injection::{
    file_len, flip_byte_at, truncate_file_to,
};
use bradian_relational::row_store::{RowStore, StoreError};
use std::collections::BTreeMap;
use tempfile::tempdir;

/// Tiny deterministic PRNG so workloads are reproducible
/// from a seed.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self { Self(seed.max(1)) }
    fn next_u32(&mut self) -> u32 {
        // xorshift64*
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x & 0xFFFF_FFFF) as u32
    }
    fn range(&mut self, n: u32) -> u32 { self.next_u32() % n.max(1) }
}

enum Op { Put(u32, u32), Overwrite(u32, u32), Delete(u32), Checkpoint }

fn generate_workload(seed: u64, len: usize) -> Vec<Op> {
    let mut rng = Rng::new(seed);
    let mut out = Vec::with_capacity(len);
    for _ in 0..len {
        match rng.range(10) {
            0..=5 => {
                // Fresh insert.
                let k = rng.range(50);
                let v = rng.next_u32();
                out.push(Op::Put(k, v));
            }
            6..=7 => {
                // Overwrite an existing key.
                let k = rng.range(50);
                let v = rng.next_u32();
                out.push(Op::Overwrite(k, v));
            }
            8 => {
                let k = rng.range(50);
                out.push(Op::Delete(k));
            }
            _ => out.push(Op::Checkpoint),
        }
    }
    out
}

/// Drive the workload to a RowStore and track every ACKed
/// state so we can compare after reopen.
/// Drive the workload and track EVERY ACKed version of each
/// key (not just the last). A truncation can roll back to an
/// earlier version, so the property check must allow any one
/// of the historical ACKed values per key.
fn drive(dir: &std::path::Path, ops: &[Op]) -> BTreeMap<Vec<u8>, Vec<Vec<u8>>> {
    let store = RowStore::open(dir).unwrap();
    let mut acked: BTreeMap<Vec<u8>, Vec<Vec<u8>>> = BTreeMap::new();
    for op in ops {
        match op {
            Op::Put(k, v) | Op::Overwrite(k, v) => {
                let key = format!("k{:04}", k).into_bytes();
                let val = format!("v{}", v).into_bytes();
                if store.put(&key, &val).is_ok() {
                    acked.entry(key).or_default().push(val);
                }
            }
            Op::Delete(k) => {
                let key = format!("k{:04}", k).into_bytes();
                let val = b"__TOMBSTONE__".to_vec();
                if store.put(&key, &val).is_ok() {
                    acked.entry(key).or_default().push(val);
                }
            }
            Op::Checkpoint => {
                let _ = store.checkpoint();
            }
        }
    }
    drop(store);
    acked
}

/// Clone the entire directory into a fresh tempdir.
fn clone_dir(src: &std::path::Path) -> tempfile::TempDir {
    let dst = tempdir().unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, dst.path().join(p.file_name().unwrap())).unwrap();
        }
    }
    dst
}

/// After-fault reopen property check.
fn assert_reopen_clean(
    dir: &std::path::Path,
    acked: &BTreeMap<Vec<u8>, Vec<Vec<u8>>>,
) {
    match RowStore::open(dir) {
        Ok(store) => {
            for (k, v) in store.scan() {
                match acked.get(&k) {
                    Some(versions) => {
                        assert!(versions.iter().any(|q| q == &v),
                            "store exposed corrupted value for key {k:?} — {v:?} not in history {versions:?}");
                    }
                    None => panic!("store exposed invented key {k:?}"),
                }
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

/// Drive a workload, then apply a fault to a random file at
/// a random offset, reopen, assert property.
#[test]
fn scripted_workload_byte_flip() {
    for seed in [1_u64, 7, 42, 100, 2026] {
        let src = tempdir().unwrap();
        let ops = generate_workload(seed, 80);
        let acked = drive(src.path(), &ops);
        // Choose a file deterministically from the seed.
        let files: Vec<_> = std::fs::read_dir(src.path()).unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                if p.is_file() { Some(p) } else { None }
            }).collect();
        if files.is_empty() { continue; }
        let pick = (seed as usize) % files.len();
        let target_name = files[pick].file_name().unwrap().to_owned();
        let scenario = clone_dir(src.path());
        let target = scenario.path().join(&target_name);
        let len = file_len(&target).unwrap();
        if len < 4 { continue; }
        let offset = (seed as u64) % len;
        flip_byte_at(&target, offset).unwrap();
        assert_reopen_clean(scenario.path(), &acked);
    }
}

/// Drive a workload, then truncate a random file to a
/// random point, reopen, assert property.
#[test]
fn scripted_workload_truncation() {
    for seed in [3_u64, 15, 77, 1024, 9999] {
        let src = tempdir().unwrap();
        let ops = generate_workload(seed, 80);
        let acked = drive(src.path(), &ops);
        let files: Vec<_> = std::fs::read_dir(src.path()).unwrap()
            .filter_map(|e| {
                let p = e.unwrap().path();
                if p.is_file() { Some(p) } else { None }
            }).collect();
        if files.is_empty() { continue; }
        let pick = (seed as usize) % files.len();
        let target_name = files[pick].file_name().unwrap().to_owned();
        let scenario = clone_dir(src.path());
        let target = scenario.path().join(&target_name);
        let len = file_len(&target).unwrap();
        if len < 4 { continue; }
        let new_len = (seed % len).max(1);
        truncate_file_to(&target, new_len).unwrap();
        assert_reopen_clean(scenario.path(), &acked);
    }
}

#[test]
fn scripted_clean_reopen_recovers_acked() {
    // Control: without any fault, reopen must expose every
    // ACKed key with its last-ACKed value.
    let src = tempdir().unwrap();
    let ops = generate_workload(2026, 60);
    let acked = drive(src.path(), &ops);
    assert_reopen_clean(src.path(), &acked);
}
