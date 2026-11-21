# ADR 0171 — DurableTypedStore retained-bytes workspace-usage exposure (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the remaining
`DurableTypedStore` strand noted in ADR 0170 §2: the
retained-history byte / version / key totals of the
catalog-aware historical SQL store are now observable
through `WorkspaceUsage`, so `GlobalBudget` enforcement
covers that engine's retained footprint.

## 1. Scope

`WorkspaceUsage` (ADR 0059 + ADR 0110) already aggregates
per-engine decoded-state counts for `PersistentCatalog`,
`MultiTableStore`, `MvccStore`, `RowStore` and
`DurableMvccStore`. The catalog-aware `DurableTypedStore`
— the backing store of `HistorySqlEngine` — was not
sampled, which left a honest-scope hole in `total_bytes()`
/ `total_rows()` for any workspace that uses historical
typed SQL.

This ADR adds:

- `WorkspaceUsage::durable_typed: Option<HistoryUsage>` —
  separate from `durable_mvcc` so operators may attach
  either or both independently.
- `WorkspaceUsage::with_durable_typed(&DurableTypedStore)`
  — one atomic reading through the store's existing
  `history_usage()` accessor, which forwards to the inner
  `DurableMvccStore` under its own state lock.
- Additive contribution to `total_bytes()` (adds
  `durable_typed.bytes`) and `total_rows()` (adds
  `durable_typed.keys`).
- Display-line extension: `… ; durable_typed: K keys / V
  versions / B bytes`.

**Default is `None`.** Callers who do not invoke
`with_durable_typed` see zero behavior change.

## 2. Out of scope

- **Automatic HistorySqlEngine admission wiring.** The
  sample at ADR 0170 for `HistorySqlEngine` intentionally
  stays at the catalog level. Wiring an engine-held
  reference to its private `DurableTypedStore` into the
  pre-statement sample would require exposing that store
  (or a sampler closure) through the engine API; that
  is a separate design choice not entangled with the
  observational surface closed here. Callers who want
  historical bytes enforced at commit may construct a
  `WorkspaceUsage` with `.with_durable_typed(..)` and
  call `GlobalBudget::check` themselves.
- **I/O and cache byte accounting.** The roadmap entry
  tracks these as separate remaining work.
- **BRPG / BRWL in-flight buffer accounting.** Out of
  scope; still tracked under G1-MEMORY.

## 3. Compliance

Extended `tests/workspace_usage.rs` (+5 tests, 19 tests
pass total):

- `with_durable_typed_reports_retained_history_counts`
  — a migrated two-row history domain exposes `keys ≥ 2`,
  `versions ≥ 2`, `bytes > 0`.
- `durable_typed_contributes_to_total_bytes_and_rows`
  — `total_bytes()` == the sampled retained bytes;
  `total_rows()` == the sampled key count.
- `durable_typed_is_additive_with_durable_mvcc_in_totals`
  — attaching a separate `DurableMvccStore` **plus** the
  typed store sums into both totals; both Display tokens
  present.
- `global_budget_rejects_on_typed_store_bytes` — a zero
  byte cap returns `ResourceError::GlobalBudget { resource:
  "total_bytes", actual: typed.bytes, … }`.
- `global_budget_rejects_on_typed_store_rows` — a
  rows cap below `typed.keys` returns `ResourceError::
  GlobalBudget { resource: "total_rows", … }`.

Clippy passes on `bradian-relational` with
`-D warnings`.

## 4. Roadmap impact

`G1-MEMORY` roadmap entry updates from:

> … automatic engine-admission wiring on every statement
> across all four SQL engines (ADR 0170); **I/O / cache
> byte accounting and DurableTypedStore retained-bytes
> exposure remain open**.

to:

> … automatic engine-admission wiring on every statement
> across all four SQL engines (ADR 0170); ADR 0171 exposes
> `DurableTypedStore` retained-history counts through
> `WorkspaceUsage::with_durable_typed`, folding into
> `GlobalBudget::check` totals; I/O / cache byte accounting
> remains open.
