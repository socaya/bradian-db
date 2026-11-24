//! ADR 0307 — Row-store write / read benchmarks.
//!
//! Scoped to single-op latency so the sample count is
//! tractable on a developer workstation: each sample is
//! one `put` (WAL fsync) or one `get` against a
//! pre-checkpointed store. Bulk-write throughput can be
//! derived by multiplication; it is deliberately NOT what
//! this bench measures.

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use bradian_relational::row_store::RowStore;
use tempfile::TempDir;

fn mk_key(i: u64) -> [u8; 16] {
    let mut k = [0u8; 16];
    k[..8].copy_from_slice(&i.to_be_bytes());
    k
}

const VALUE: &[u8; 100] = &[0x42; 100];

fn single_put_latency(c: &mut Criterion) {
    // Each sample: open a fresh store + append one entry.
    // iter_batched_ref so the setup (store creation) is
    // not timed and the store is dropped between samples.
    c.bench_function("row_store_put_fresh", |b| {
        b.iter_batched_ref(
            || {
                let dir = TempDir::new().unwrap();
                let store = RowStore::open(dir.path()).unwrap();
                (dir, store, 0u64)
            },
            |(_dir, store, i)| {
                store.put(black_box(&mk_key(*i)), black_box(VALUE)).unwrap();
                *i += 1;
            },
            BatchSize::SmallInput,
        );
    });
}

fn single_get_hot_latency(c: &mut Criterion) {
    // Pre-seed + checkpoint so get() measures the
    // checkpoint-tree + delta read path, not WAL replay.
    let dir = TempDir::new().unwrap();
    let store = RowStore::open(dir.path()).unwrap();
    for i in 0..10_000u64 { store.put(&mk_key(i), VALUE).unwrap(); }
    store.checkpoint().unwrap();

    let mut i: u64 = 0;
    c.bench_function("row_store_get_checkpointed", |b| {
        b.iter(|| {
            let key = mk_key(i % 10_000);
            i = i.wrapping_add(1);
            black_box(store.get(black_box(&key)));
        });
    });
}

criterion_group!(benches, single_put_latency, single_get_hot_latency);
criterion_main!(benches);
