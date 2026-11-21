# Master Gate 1 — DurableTypedStore retained-bytes workspace-usage verification

21 November 2026. ADR 0171. Observability + admission-coverage slice;
**non-admitting by default**. Gate 1 remains active.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test workspace_usage` | 19 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

The gap register (`BRADIAN_IMPLEMENTATION_GAPS.md` G1-MEMORY row
and `BRADIAN_MASTER_ROADMAP.md`) marked
`DurableTypedStore` retained-bytes exposure open after
ADR 0170 wired automatic engine-admission of
`GlobalBudget` into every SQL engine. The catalog-aware
historical typed store — `HistorySqlEngine`'s backing
store — contributes retained history to the workspace
footprint but was not sampled by `WorkspaceUsage`, so
`GlobalBudget::check` under-counted historical SQL
usage.

Change (ADR 0171):

- New field `WorkspaceUsage::durable_typed:
  Option<HistoryUsage>`, independent from `durable_mvcc`.
- `WorkspaceUsage::with_durable_typed(&DurableTypedStore)`
  builder: one atomic reading via the store's existing
  `history_usage()` (forwards to inner `DurableMvccStore`
  under its state lock).
- `total_bytes()` adds `durable_typed.bytes`;
  `total_rows()` adds `durable_typed.keys`.
- `Display` extends with `… ; durable_typed: K keys / V
  versions / B bytes`.
- Default is `None`; callers that do not attach the
  store see zero behavior change.

Added tests (5 new in `tests/workspace_usage.rs`, 19 total
in this test binary):

1. `with_durable_typed_reports_retained_history_counts` —
   migrated two-row history domain exposes
   `keys ≥ 2`, `versions ≥ 2`, `bytes > 0`.
2. `durable_typed_contributes_to_total_bytes_and_rows` —
   totals equal the sampled retained counts.
3. `durable_typed_is_additive_with_durable_mvcc_in_totals`
   — attaching both engines sums into both totals; both
   Display tokens present.
4. `global_budget_rejects_on_typed_store_bytes` — zero
   byte cap returns
   `ResourceError::GlobalBudget { resource: "total_bytes",
    actual: typed.bytes, … }`.
5. `global_budget_rejects_on_typed_store_rows` — rows cap
   below `typed.keys` returns
   `ResourceError::GlobalBudget { resource: "total_rows",
    … }`.

## Scope limits (honest)

- No automatic HistorySqlEngine admission wiring. Callers
  who want historical bytes enforced at commit must
  construct a `WorkspaceUsage` with `with_durable_typed`
  and call `GlobalBudget::check` explicitly. Engine-held
  automatic sampling is an API design question deferred
  to a follow-up ADR.
- No I/O or cache byte accounting. Still tracked under
  G1-MEMORY.
- No BRPG / BRWL in-flight buffer accounting.

## Evidence

- Source: `crates/bradian-relational/src/workspace_usage.rs`
  (new field, builder, updated Display and totals).
- Tests:
  `crates/bradian-relational/tests/workspace_usage.rs`
  (open_migrated_typed_store helper + 5 new
  `durable_typed` tests).
- ADR: `docs/adr/0171-durable-typed-workspace-usage.md`.
