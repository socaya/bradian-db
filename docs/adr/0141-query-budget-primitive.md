# ADR 0141 — Query budget primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Companion to ADR 0114
(lock-acquisition timeout policy) and ADR 0041/0044
(scan-admission caps). Provides a per-QUERY budget object
the engine can consult at admission checkpoints.

## 1. Scope

New module `bradian_relational::query_budget`:

- `QueryBudget` — four optional knobs:
  - `max_duration: Option<Duration>` (wall clock).
  - `max_rows_scanned: Option<u64>`.
  - `max_bytes_materialized: Option<u64>`.
  - `max_iterations: Option<u64>`.
- `QueryBudget::default()` — conservative: 30 s, 10 M
  rows, 1 GiB materialized, 1 G iterations.
- `QueryBudget::unbounded()` — all four None.
- `BudgetTracker` — stateful; captures `Instant` at
  construction.
  - `charge_rows(n)` / `charge_bytes(n)` /
    `charge_iteration()` — add + check; saturating.
  - `check_deadline()` — wall-clock only; cheap enough
    for inner loops.
  - Accessors: `rows_scanned`, `bytes_materialized`,
    `iterations`, `elapsed`.
- `BudgetExceeded { Duration / Rows / Bytes /
  Iterations }` — one variant per knob.

## 2. Out of scope (follow-ups)

- **Cancellation / interrupt** — breaching the budget
  returns an error at the next charge point only; a
  running tight loop without a charge point won't be
  interrupted mid-iteration.
- **Per-engine integration** — the SQL engines don't
  consult `BudgetTracker` yet. Each engine needs its
  own `charge_*` placement ADR.
- **Hierarchical budgets** — parent/child plan nodes
  can share a parent tracker via `Arc`, but no
  explicit API for hierarchical budgets.
- **CPU-time budget** — needs libc `getrusage`;
  wall-clock only today.
- **Priority / fair scheduling** — budgets are
  independent, not queued.

## 3. Compliance

- Default budget has all four bounds set.
- `unbounded()` has all four None.
- Row / byte / iteration charges succeed under budget
  and error with the correct variant over budget.
- `check_deadline()` fires after sleep over the max.
- Unbounded tracker never errors (even at `u64::MAX`
  values).
- Accessors report cumulative state.
- `saturating_add` prevents overflow panic.
- 11 new tests in `query_budget::tests`.

## 4. Honest status

Foundation only. **BRD-RUNTIME-003 query-level timeout
+ resource bound** is not yet satisfied because no SQL
engine consults the tracker. A future per-engine ADR
places `charge_*` calls at scan admission, buffer-pool
fetch, hash-join build, and sort-merge points.
