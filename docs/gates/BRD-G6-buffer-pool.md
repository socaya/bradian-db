# BRD-G6 — Generation-scoped immutable physical page cache

**SRS:** Bradian Master Architecture and SRS v1.1  
**ADR:** [0011 — Generation-scoped buffer pool](../adr/0011-generation-scoped-buffer-pool.md)  
**Branch:** `main`  
**Status:** Source and tests committed; manually verified build/test evidence required before gate acceptance.

## Scope implemented

Bradian's actual B+tree and checkpoint files are read through a bounded, checksum-verifying LRU buffer pool, not placeholder interfaces.

The `PageCache` implementation accepts a file, expected checkpoint LSN and maximum number of pages. It reads only valid fixed-size BRPG pages, verifies CRC/page ID/generation on cache misses, retains decoded pages, evicts least-recently-used entries at capacity and exposes cache statistics.

`DiskBTree` uses that cache for repeated tree lookups. The WAL-backed `RowStore` retains both the index and snapshot-page caches across indexed queries. A new checkpoint explicitly clears both caches. Newer WAL commits bypass the older checkpoint index, maintaining read freshness. The existing authoritative row-map comparison remains in place as a defensive integrity check.

## Source tests added or adapted

- [x] Positive capacity required.
- [x] Bounded LRU residency and deterministic eviction.
- [x] Wrong physical page identity fails even when the CRC is valid.
- [x] Wrong checkpoint LSN rejected.
- [x] Non-page-aligned physical file rejected.
- [x] Corrupt physical pages never enter the cache.
- [x] Repeated primary-key queries reuse cached index/data pages.
- [x] Committed WAL mutations bypass an older checkpoint.
- [x] New checkpoint generation invalidates old caches.
- [x] Previous B+tree tests updated for explicit cache error boundary.

These are committed tests; their execution has not been verified.

## Manual acceptance required

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Additional qualification remains necessary for power-loss recovery, file mutation by noncooperating processes, platform-specific file-handling rules, cache memory benchmarking, large-range reads and scale under contention.

## Explicit non-goals

No dirty page/write-back handling, in-place B+tree split/merge, WAL-before-data page flush, page eviction under active pins, concurrent page latches, buffer admission/priority, page-level encryption or MVCC snapshots is claimed. These are the next storage/transaction milestones and must not be described as implemented.
