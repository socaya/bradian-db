# ADR 0172 — RowStore page-cache byte accounting via `WorkspaceUsage` (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the "I/O /
cache byte accounting" strand of G1-MEMORY (the second
and last code-only strand noted in ADR 0170 §2 and ADR
0171 §2). After ADRs 0170/0171 the only in-process
decoded-byte footprint that `WorkspaceUsage::total_bytes()`
did not fold in was the ADR 0011 generation-scoped
read-only page caches held by every `RowStore`.

## 1. Scope

- `CacheStats::cached_bytes(&self) -> usize` — exact
  reading: `resident.saturating_mul(PAGE_SIZE)`. Every
  `PageCache` entry holds one decoded `Page` whose
  memory footprint is `PAGE_SIZE` (4096 bytes).
- `RowStore::page_cache_bytes(&self) -> usize` — single
  reading under the store mutex, summing the derived-
  index B+tree cache bytes and the snapshot-page cache
  bytes for the current checkpoint generation.
- `WorkspaceUsage::page_cache_bytes: Option<usize>` —
  new optional field, independent from `row_store`.
- `WorkspaceUsage::with_row_store_caches(&RowStore)` —
  one-atomic-reading builder, mirroring every other
  per-engine composer.
- `total_bytes()` folds `page_cache_bytes` in; Display
  extends with `… ; page_cache: B bytes`.

**Default is `None`.** Callers who do not invoke
`with_row_store_caches` see zero behavior change.
Operators who do not want cache bytes enforced against
the global cap — they are refillable and transient —
simply do not attach the reading.

## 2. Out of scope

- **In-flight WAL / checkpoint I/O buffers.** These
  are transient (constructed, synced, dropped on commit
  / checkpoint), not retained between operations. No
  separate steady-state byte footprint to meter.
- **Cross-generation cumulative cache history.** A new
  checkpoint generation invalidates both caches by
  design (see
  `indexed_row_store_reuses_generation_caches_and_invalidates_on_checkpoint`
  at `crates/bradian-relational/tests/buffer_pool.rs`).
  `page_cache_bytes` reflects the current generation
  only, which is the intended steady-state footprint.
- **Variable page sizes.** `PAGE_SIZE` is fixed at
  compile time; a future BRPG v2 larger-page format
  would require only `cached_bytes()` to multiply by
  the registered page size instead.

## 3. Compliance

Extended `tests/workspace_usage.rs` (+6 tests, 25 tests
pass total in this binary):

- `page_cache_bytes_is_zero_before_any_checkpoint_reads`
  — a fresh `RowStore` reports `Some(0)` from
  `with_row_store_caches`.
- `page_cache_bytes_reports_non_zero_after_indexed_reads`
  — after a checkpoint and three indexed reads, cache
  bytes are positive and a multiple of `PAGE_SIZE`.
- `page_cache_bytes_folds_into_total_bytes` —
  `total_bytes()` equals
  `row_bytes + delta_bytes + page_cache_bytes`.
- `page_cache_bytes_invalidates_on_new_checkpoint` —
  after a new checkpoint, the sampled cache bytes drop
  to `Some(0)`.
- `global_budget_rejects_on_page_cache_bytes` — zero
  byte cap returns
  `ResourceError::GlobalBudget { resource: "total_bytes",
    actual: page_cache_bytes, … }`.
- `cached_bytes_on_cache_stats_is_resident_times_page_size`
  — unit test on the new `CacheStats::cached_bytes`
  accessor.

Clippy clean on `bradian-relational` with
`-D warnings`.

## 4. Roadmap impact

`G1-MEMORY` roadmap entry closes its remaining
code-writable strand:

> … automatic engine-admission wiring on every statement
> across all four SQL engines (ADR 0170); ADR 0171
> exposes `DurableTypedStore` retained-history counts
> through `WorkspaceUsage::with_durable_typed`; **ADR
> 0172 adds `CacheStats::cached_bytes()`,
> `RowStore::page_cache_bytes()` and
> `WorkspaceUsage::with_row_store_caches(..)` so page-
> cache bytes fold into `GlobalBudget::check` totals**.
> Automatic HistorySqlEngine-held sampling of its own
> typed store remains an API design question.

The remaining G1-MEMORY work is qualification
(memory-pressure + failure-injection scenarios on real
hardware), not code-writable.
