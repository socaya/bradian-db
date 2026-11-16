# ADR 0110 — Workspace-wide admission budget (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Addresses the G1-MEMORY
"cross-engine totals and global budgets remain open" item
from the master roadmap.

## 1. Honest scope

`WorkspaceUsage` already aggregates per-engine counts
(ADR 0059 + extension). ADR 0110 adds:

- `WorkspaceUsage::total_bytes()` — sums bytes-in-use across
  `row_store` (row + delta bytes) and `durable_mvcc`
  (retained-history bytes). Catalog / domain / in-memory
  MVCC counts are different semantic units and do not
  contribute.
- `WorkspaceUsage::total_rows()` — sums row-store rows +
  durable-MVCC retained keys.
- `GlobalBudget { max_total_bytes, max_total_rows }` with
  factory helpers `unlimited()`, `with_max_total_bytes(cap)`,
  `with_max_total_rows(cap)`.
- `GlobalBudget::check(&WorkspaceUsage)` returns
  `ResourceError::GlobalBudget { resource, actual, limit }`
  on cap exceeded; byte cap fires before row cap.
- New `ResourceError::GlobalBudget` variant.

**Opt-in enforcement.** Default `GlobalBudget` has `None`
caps and never rejects. Callers who want a workspace ceiling
construct one with explicit caps and invoke `check()` after
sampling `WorkspaceUsage`.

## 2. Out of scope

- **Automatic enforcement in engine admission paths.** Global
  budget is a post-sampling check; the engine admission paths
  continue to use their per-engine limits. A future ADR could
  wire the global check into a central admission controller.
- **Atomic sample-and-check.** `check()` runs against a
  frozen snapshot; a concurrent mutation between sample and
  use is not covered (same contract as `WorkspaceUsage`
  itself per ADR 0059).
- **Memory pressure response** (shrink, LRU eviction, back-
  pressure signals) — those require a design on top of the
  diagnostic signal this ADR provides.
- **Cross-process total budget.** In-process only.

## 3. Compliance

1. `total_bytes()` sums `row_store.row_bytes + delta_bytes +
   durable_mvcc.bytes`.
2. `total_rows()` sums `row_store.rows + durable_mvcc.keys`.
3. Default `GlobalBudget` never rejects.
4. `max_total_bytes=0` rejects any non-zero usage with
   `{ resource: "total_bytes", actual, limit }`.
5. `max_total_rows=N` where actual > N rejects with
   `{ resource: "total_rows", actual, limit }`.
6. When both caps are exceeded, byte cap fires first.
7. Frozen snapshot does not track post-sample mutations.
8. 14 tests pass (8 pre-ADR-0110 + 6 new).
