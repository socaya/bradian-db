# ADR 0308 — Mutable physical page storage: skeleton

## Status

Accepted as a skeleton. First slice of P2.

## Context

Gate 1's row store uses immutable disk B+tree checkpoints
plus a WAL-backed overlay (ADR 0012, ADR 0014). P2 calls
for **mutable physical page storage with a buffer manager**
— the foundation on which distributed commit, PITR,
consensus replication and schema compaction are built.

A full buffer-pool + dirty-page-manager + WAL-protected
page write path is a Gate-sized effort. The responsible
thing to ship as the first P2 slice is the **skeleton**:
the typed public surface future slices will fill in,
exercised end-to-end by an in-memory backing so the
invariants are observable today.

## Decision

New module `bradian_relational::page_buffer` — a
buffer-pool / dirty-page-manager layer **above** the
existing `mutable_page` file allocator (ADR 0096) — exposing:

```rust
pub struct PageId(pub u64);
pub struct Page { pub id: PageId, pub lsn: u64, pub bytes: Vec<u8> }
pub trait PageStore {
    fn allocate(&self) -> StoreResult<PageId>;
    fn read   (&self, id: PageId)        -> StoreResult<Page>;
    fn write  (&self, page: Page)        -> StoreResult<()>;    // WAL append + dirty mark
    fn flush  (&self, up_to_lsn: u64)    -> StoreResult<()>;    // force durable
    fn checkpoint(&self)                 -> StoreResult<u64>;   // snapshot + truncate WAL
}
pub struct InMemoryPageStore { ... }   // implements PageStore today
```

The `InMemoryPageStore` holds pages in a mutex-protected
map, records every write to a Vec-backed WAL and honours
the fence semantics the eventual disk-backed implementation
must preserve:

- `write` admits a page's new LSN only if it monotonically
  advances the page's prior LSN (prevents stale replays).
- `flush(up_to_lsn)` is a no-op in-memory but recorded in
  statistics so a test can assert flush calls happen.
- `checkpoint` snapshots the current page set, assigns a
  new global LSN and truncates the WAL conceptually.

Honest scope:

- **In-memory only.** No disk pages, no real buffer pool,
  no page replacement policy, no `pread`/`pwrite`. The
  eventual real backing is a separate slice (ADR 0317
  PITR builds on a disk-backed version).
- No multi-writer concurrency control beyond the global
  mutex.
- No reference to Gate-1 row-store files; this is a new,
  parallel subsystem.
- 1 skeleton test documents the fence + LSN invariants
  the real backing must preserve.

## Consequences

- The typed surface is now present so future P2 slices
  (consensus, distributed commit, PITR) can be designed
  against it.
- A regression in the LSN-fence invariant is caught in
  the skeleton long before the disk backing exists.
- 3 unit tests: allocate → write → read round-trips,
  monotonic-LSN fence on repeated writes, checkpoint
  returns a new globally-advancing LSN.
- **This slice does NOT claim mutable physical pages are
  implemented.** It ships the type surface and the
  invariants, nothing more.
