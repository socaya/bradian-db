# ADR 0170 — Workspace budget automatic admission wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Closes the one
code-writable Gate 1 blocker that remained on the
roadmap after Batches A–L reached 100%: automatic
workspace-wide admission wiring of ADR 0110's
`GlobalBudget` into every SQL engine.

## 1. Scope

`GlobalBudget` previously had to be sampled + checked
by the caller after the fact (see ADR 0110 §2). This
ADR adds an opt-in field on each SQL engine and
executes the check **before every statement**.

New field + constructor + accessor on all four engines:

| Engine | field | setter | sample composition |
|---|---|---|---|
| `SqlEngine` | `global_budget: GlobalBudget` | `with_global_budget(budget)` | `WorkspaceUsage::for_catalog(&self.catalog)` |
| `DomainSqlEngine` | same | same | `.for_catalog(..).with_domain(&self.domain)` |
| `MvccSqlEngine` | same | same | `.for_catalog(..)` + `mvcc_distinct_keys` / `mvcc_total_versions` populated from `TypedMvccStore::stats()` |
| `HistorySqlEngine` | same | same | `.for_catalog(..)` (durable-history byte totals stay unexposed) |

Each engine samples + calls `self.global_budget.check(&usage)?` at the top of its statement execution path (`execute_impl` in `SqlEngine`, `execute_inner` in the three session engines). A breach returns `SqlError::Resource(ResourceError::GlobalBudget { … })`.

**Default is unlimited.** `GlobalBudget::default()` has
`None` on every cap; the check never rejects. Existing
callers who do not call `with_global_budget` see zero
behavior change — this is strictly opt-in.

## 2. Out of scope

- **Mid-statement resampling.** The check runs once per
  statement, not per row. Per-row admission is the
  job of `QueryBudget` (ADR 0141).
- **I/O / cache byte accounting.** The roadmap entry
  notes "I/O/cache … of the global budget remain open".
  This ADR closes the engine-admission wiring; I/O
  and cache accounting are a separate instrumentation
  effort.
- **DurableTypedStore byte totals.** HistorySqlEngine's
  sample uses catalog counts only because the retained-
  history byte totals are not exposed on `DurableTypedStore`.
  A follow-up ADR can add a `retained_bytes()` accessor
  and plumb it through.

## 3. Compliance

New integration test file `sql_engine_global_budget.rs`
(7 tests):
- `unlimited_default_never_rejects` — full DDL + DML +
  SELECT under `GlobalBudget::unlimited()`.
- `engine_without_with_global_budget_call_behaves_as_unlimited`
  — sanity check that the default is opt-in.
- `with_global_budget_sets_the_cap`.
- `breach_during_statement_execution_returns_sql_error_resource`.
- `global_budget_check_via_breach_path` — direct
  exercise of `GlobalBudget::check` to prove the
  wiring path returns the correct
  `ResourceError::GlobalBudget` variant.
- `error_propagates_as_sqlerror_resource`.
- `builder_style_composes_cleanly`.

All four engines compile cleanly; the full workspace
test suite remains green.

## 4. Roadmap impact

`G1-MEMORY` roadmap entry updates from:

> ADR 0110 adds `WorkspaceUsage::total_bytes()`/`total_rows()`
> + opt-in `GlobalBudget` workspace-wide admission cap;
> **I/O/cache and automatic engine-admission wiring of the
> global budget remain open**

to:

> … `GlobalBudget` workspace-wide admission cap with
> **automatic engine-admission wiring on every statement
> across all four SQL engines (ADR 0170)**; I/O / cache
> byte accounting and DurableTypedStore retained-bytes
> exposure remain open.
