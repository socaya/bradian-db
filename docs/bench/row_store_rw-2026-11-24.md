# Row-store write / read benchmark — 2026-11-24

**ADR 0307** — reference numbers for `RowStore::put` and
`RowStore::get` on the Gate-1 WAL-backed row store.

## Hardware / environment

| Item | Value |
|------|-------|
| Platform  | Darwin 25.2.0 arm64 |
| CPU       | Apple M4 Pro, 12 logical cores |
| Memory    | 24 GiB |
| Filesystem| APFS (SSD) |
| Rust      | `rustc 1.92.0 (ded5c06cf 2025-12-08)` |
| Commit    | `f1c7c2d` (post-0306, pre-0307) |
| Criterion | 0.5.1, `--sample-size 20 --measurement-time 5` |

## Workload

| Case | Description |
|------|-------------|
| `row_store_put_fresh`       | Each sample opens a fresh `RowStore`, puts one 100-byte value under a 16-byte key. Measures a WAL-fsynced append + index-overlay insert. |
| `row_store_get_checkpointed`| Pre-seeded store with 10 000 entries + `checkpoint()`. Each sample is one `get` against a randomised hot key. Measures the checkpoint B+tree + delta lookup path, with the OS buffer cache warm. |

## Results

| Case | Median | Low | High |
|------|-------:|----:|-----:|
| `row_store_put_fresh`        | **4.76 ms** | 4.44 ms | 5.17 ms |
| `row_store_get_checkpointed` | **73.6 ns** | 73.1 ns | 74.2 ns |

Observations:

- The 4.76 ms `put` median is dominated by the WAL
  `fsync`; the row-level work is in the hundreds of
  microseconds. Expect sub-millisecond `put` on
  non-synchronous backends.
- Hot `get` latency sits in the 70-ns range — one
  checkpoint-tree descent plus a delta-overlay check on
  cached pages. No syscalls.

Numbers are reproducible via:

```sh
cargo bench -p bradian-benchmark --bench row_store_rw \
    -- --sample-size 20 --measurement-time 5
```

## Caveats

- Single-op latency; bulk throughput is not reported here.
  A grouped put bench is a straightforward extension of
  `row_store_rw.rs` using `iter_batched` with a shared
  store across the batch.
- These numbers are **laptop-class**. Published production
  numbers belong to a regulated environment per spec §12
  (`BenchContext`): hardware, dataset size, concurrency,
  cache state, commit SHA.
