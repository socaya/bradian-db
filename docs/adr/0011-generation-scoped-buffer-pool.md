# ADR 0011 — Generation-scoped read-only buffer pool

**Bradian Database Systems · Master Architecture & SRS v1.1**  
**Status:** Implemented as a bounded immutable-page read cache; writable dirty-page buffer management remains a separate gate.  
**Supersedes:** None. Complements [ADR 0010](0010-checkpoint-btree.md) and [ADR 0008](0008-native-row-wal-checkpoint.md).

## Context

The existing Bradian relational storage engine synchronizes committed WAL mutations and publishes complete physical checkpoints. The immutable B+tree index introduced in G5 is derived from these checkpoints. Previously, every indexed point query reopened the published index and snapshot files and reread physical pages. A large ordered range query could reopen and reread the same snapshot data page for each row, despite their physical colocation.

The database now requires a bounded, deterministic physical-page caching layer without weakening WAL and checkpoint guarantees or introducing an incomplete writable buffer manager.

## Decision and implementation

A new `bradian-relational::buffer_pool::PageCache` owns an open file descriptor for **one immutable physical file generation**, identified by its expected checkpoint log sequence number. The caller configures a positive maximum number of resident pages; the default B+tree and row checkpoint caches are each bounded to 128 decoded pages (each underlying physical page is 4096 bytes). The module uses a `BTreeMap` of cached page IDs and monotonic logical access timestamps to choose the least-recently-used victim. The eviction work is linear in the bounded cache capacity, a deliberate implementation choice rather than an unbounded per-lookup allocation.

On every miss, `PageCache` seeks to `page_id × PAGE_SIZE`, reads exactly one physical page, and validates it through the existing `BRPG` decoder **before** caching or exposing any data. The page identity and generation LSN must agree with the file offset and the expected snapshot. CRC failures, unsupported format versions, bad physical lengths and wrong page identities are errors, not cache misses or absent rows.

On hits, an owned clone of the already verified decoded page is returned. The cache never leaks a reference to an evictable entry, and its stored pages remain safe because published checkpoint and index files are immutable by contract. This must not be reused for writable pages without adding write latches, physical LSN fencing, double-write/torn-page recovery and write-ahead flushing.

## Integration

**B+tree reader:** `DiskBTree::open_with_capacity(path, lsn, capacity)` creates a generation-bound `PageCache`, validates page 0, and keeps index-node pages resident through point and range lookups. `DiskBTree::open` applies the default capacity of 128. Cached pages maintain existing bounds and structural validation.

**Row store:** `RowStore::Inner` retains one `DiskBTree` and one snapshot `PageCache` for the current committed checkpoint generation. Both are accessed under the existing `parking_lot::Mutex`, so data reads and checkpoint publication do not race on shared file descriptors or mutable cache state. `indexed_get` uses the resident B+tree for pointer resolution and the resident snapshot cache for row data. `indexed_range` similarly avoids reopening snapshot files for each row. The final returned values are still checked against the authoritative committed row map.

**Freshness:** If the latest committed WAL LSN exceeds the checkpoint LSN, indexed reads bypass the older physical B+tree and return the synchronized in-memory committed version instead. On publishing a new checkpoint, `inner.index` and `inner.snapshot_pages` are invalidated so no page from the previous generation can be presented as current.

**Observability:** `PageCache::stats` reports hits, misses, evictions, resident pages and capacity; `DiskBTree::cache_stats` exposes index activity; and `RowStore::index_cache_stats` exposes the current index and snapshot cache counters when available. These metrics are diagnostic and do not claim a general-purpose metrics registry.

## Memory, consistency and security properties

- Resident pages are strictly bounded by the configured capacity per cache. Every insert at capacity evicts one least-recently-used page.
- A corrupted, previously uncached page is never inserted into the cache. A cache hit is safe only while files remain immutable and their identities cannot be replaced without rebuilding the owning cache.
- The reader rejects a page whose stored ID or checkpoint LSN differs from its expected position/generation, even if its physical CRC is otherwise valid.
- Cache entries are invalidated when a new checkpoint generation becomes authoritative, not just when another file path is selected.
- The authoritative WAL remains the commit record. No dirty page writes or page flush decisions are made in this buffer pool.
- Read-side caching under the coarse row-store mutex improves repeated physical I/O but is **not** a high-concurrency, lock-striped, pin-counted transactional buffer pool.
- Caches hold decoded pages, not encryption keys; at-rest encryption and secure memory wiping are still outstanding security requirements.

## Evidence and limitations

Source: `crates/bradian-relational/src/buffer_pool.rs`, `btree.rs`, and `row_store.rs`. Tests in `tests/buffer_pool.rs` exercise deterministic LRU behavior, capacity, wrong LSN and ID, CRC rejection, page-size validation, repeated indexed lookups and generation invalidation. Existing B+tree tests have been updated to expect the cache integrity error boundary.

This is **not** an in-place B+tree update manager, a write-back buffer cache, MVCC, record latching, a replacement for fsync, or an admission-control system. Those capabilities require new ADRs, fault-injection testing and explicit acceptance gates.

Automatic GitHub Actions remain disabled at the owner's request. Code and tests were committed to `main`; their execution has not been independently verified.
