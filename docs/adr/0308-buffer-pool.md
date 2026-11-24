# ADR 0308 — Buffer-pool / dirty-page-manager

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
disk-backed buffer pool** layered over the existing
`mutable_page` allocator (ADR 0096), with LRU eviction,
dirty-page tracking, and flush/checkpoint persistence.

## Context

The skeleton exposed `InMemoryPageStore` with the LSN
fence invariant but no disk backing. A real buffer
manager needs to cache pages, evict them under pressure
without losing writes, and offer a flush/checkpoint
contract that an upper layer (B+tree, transaction
manager) can rely on.

## Decision

Module `bradian_relational::page_buffer`:

```rust
pub trait PageStore { allocate, read, write, flush, checkpoint }
pub struct InMemoryPageStore;   // preserved — tests
pub struct DiskPageBuffer;      // real — LRU + dirty + file
```

`DiskPageBuffer`:

- Wraps a `MutablePageFile` (ADR 0096 file allocator).
- Holds an LRU of cached pages bounded by a caller-chosen
  `capacity: NonZeroUsize`.
- Tracks `dirty` per cached page.
- On `read` miss, reads from the file into the cache (and
  evicts if the pool is over capacity).
- On `write`: loads the page into cache first (so the
  per-page monotonic LSN fence is checked against the
  real prior value), rejects stale-LSN writes, updates
  the cache, marks dirty, touches LRU.
- On **eviction**, a dirty victim is flushed to the file
  BEFORE being dropped from the cache, so no committed
  write is ever lost to eviction pressure.
- `flush(up_to_lsn)` writes every dirty cache entry with
  `lsn <= up_to_lsn` back to the file and clears the
  dirty bit on each.
- `checkpoint()` calls `flush(u64::MAX)` and advances a
  global LSN counter; pages written before a checkpoint
  survive a reopen.

## Scope

**In scope:**
- Real LRU-bounded disk-backed buffer pool.
- Dirty-tracked flush and checkpoint.
- Per-page monotonic LSN fence (unchanged from skeleton
  — but now also enforced against on-disk state via a
  load-into-cache step before write).
- Eviction flushes dirty pages before dropping them.
- Page bytes survive reopen after checkpoint.
- 8 unit tests: 2 preserved in-memory tests + 6 new
  disk-backed ones covering round-trip, LSN fence,
  eviction-flushes-dirty (3-page workload on cap=2),
  selective flush by LSN, checkpoint flushes all + bumps
  LSN, reopen preserves written bytes.

**Out of scope** (dedicated follow-ups):
- WAL-protected dirty-page writes (dirty pages are
  written on flush but not journaled; a real crash
  between write and flush loses the write).
- Fine-grained concurrency (the pool uses a single
  mutex; a sharded cache + per-page locks is the
  sensible next slice).
- Pin / unpin for long-running operations.
- Page kinds beyond `Opaque`.

## Consequences

- `DiskPageBuffer` is the first real B+tree-friendly
  page manager in the tree. An upper layer can now
  allocate pages, write them with monotonic LSNs, and
  trust that `flush`/`checkpoint` land them durably.
- The 2026-11-24 "SKELETON" status on 0308 is retired.
- **ALL 12 P2 slices (0308–0319) have now been promoted
  from skeleton to real implementation.**
