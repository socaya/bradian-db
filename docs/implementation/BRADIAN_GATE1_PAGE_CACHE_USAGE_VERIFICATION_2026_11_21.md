# Master Gate 1 — RowStore page-cache workspace-usage verification

21 November 2026. ADR 0172. Observability + admission-coverage
slice; **non-admitting by default**. Gate 1 remains active.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test workspace_usage` | 25 passed, 0 failed |
| `cargo test --locked -p bradian-relational --test buffer_pool` | 6 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

The gap register (`BRADIAN_IMPLEMENTATION_GAPS.md` G1-MEMORY row and
`BRADIAN_MASTER_ROADMAP.md`) called out "I/O / cache byte accounting"
as the remaining G1-MEMORY code strand after ADRs 0170 (automatic
engine-admission wiring) and 0171 (DurableTypedStore exposure). The
only decoded-byte footprint `WorkspaceUsage::total_bytes()` did not
fold in was the ADR 0011 read-only page caches owned by every
`RowStore`: the derived-index B+tree cache and the snapshot-page
cache.

Change (ADR 0172):

- New `CacheStats::cached_bytes(&self) -> usize` helper:
  `resident.saturating_mul(PAGE_SIZE)`. One decoded `Page` per
  resident entry, each `PAGE_SIZE` (4096 bytes).
- New `RowStore::page_cache_bytes(&self) -> usize` under the store
  mutex, summing both caches for the current checkpoint generation.
- New `WorkspaceUsage::page_cache_bytes: Option<usize>` field.
- New `WorkspaceUsage::with_row_store_caches(&RowStore)` builder.
- `total_bytes()` folds `page_cache_bytes` in; `Display` extends
  with `… ; page_cache: B bytes`.
- Default is `None`; callers who do not attach the reading see
  zero behavior change.

Added tests (6 new in `tests/workspace_usage.rs`, 25 total):

1. `page_cache_bytes_is_zero_before_any_checkpoint_reads` — fresh
   `RowStore` reports `Some(0)`.
2. `page_cache_bytes_reports_non_zero_after_indexed_reads` — after
   a checkpoint and three indexed reads, bytes > 0 and a multiple
   of `PAGE_SIZE`.
3. `page_cache_bytes_folds_into_total_bytes` — `total_bytes()` ==
   `row_bytes + delta_bytes + page_cache_bytes`.
4. `page_cache_bytes_invalidates_on_new_checkpoint` — after a new
   checkpoint, cache bytes drop to `Some(0)`.
5. `global_budget_rejects_on_page_cache_bytes` — zero byte cap
   returns
   `ResourceError::GlobalBudget { resource: "total_bytes",
    actual: page_cache_bytes, … }`.
6. `cached_bytes_on_cache_stats_is_resident_times_page_size` —
   unit test on the new `CacheStats::cached_bytes` accessor.

The existing ADR 0011 cache-invalidation invariants
(`indexed_row_store_reuses_generation_caches_and_invalidates_on_checkpoint`)
continue to pass, confirming that `page_cache_bytes()` reads only
post-publication immutable state and does not race with checkpoint
generation turnover.

## Scope limits (honest)

- No in-flight WAL / checkpoint I/O buffer metering. These are
  transient and dropped on commit/checkpoint; there is no retained
  steady-state footprint.
- No automatic HistorySqlEngine-held sampling of its own typed
  store (ADR 0171 §2).
- No memory-pressure or failure-injection qualification. Those
  remain under G1-MEMORY qualification, which is an infrastructure
  item.

## Evidence

- Source:
  `crates/bradian-relational/src/buffer_pool.rs` (added
  `CacheStats::cached_bytes`),
  `crates/bradian-relational/src/row_store.rs` (added
  `RowStore::page_cache_bytes`),
  `crates/bradian-relational/src/workspace_usage.rs` (new
  optional field, builder, updated Display + totals).
- Tests:
  `crates/bradian-relational/tests/workspace_usage.rs` (6 new
  tests).
- ADR: `docs/adr/0172-page-cache-workspace-usage.md`.
