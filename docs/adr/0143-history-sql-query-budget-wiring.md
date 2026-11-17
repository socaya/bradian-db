# ADR 0143 — History SQL engine query-budget wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** First engine-level
integration of the ADR 0141 `BudgetTracker`. Promotes the
query-budget primitive from a standalone module to an
observable effect on real SQL execution.

## 1. Scope

Three narrow edits:

1. **`SqlError::Budget(BudgetExceeded)`** — new variant
   on the existing `SqlError` enum with `#[from]` so any
   `BudgetTracker::charge_*` or `check_deadline()` error
   propagates with `?`.
2. **`HistorySqlSession.budget: Option<BudgetTracker>`**
   — new optional field. `None` preserves the pre-ADR
   behavior exactly.
3. **`HistorySqlEngine::with_session_budget(budget, cb)`**
   — new constructor that opens a session with a
   `BudgetTracker::new(budget)`. Existing constructors
   (`with_session`, `with_session_control`,
   `with_session_limits`) leave `budget = None` and
   behave unchanged.

SELECT-path instrumentation (in `HistorySqlSession::
execute_inner` under the `"select"` arm):

- **Before scan:** `budget.check_deadline()?`.
- **After materializing rows, before windowing:**
  `budget.charge_rows(rows.len() as u64)?` +
  `budget.check_deadline()?`.

The window/projection path keeps the existing
`ScanControl.check()` calls; nothing is removed.

## 2. Out of scope (follow-ups)

- **Other engines** — `SqlEngine` (single-process),
  `MvccSqlEngine`, `DomainSqlEngine` do not yet
  consult the budget. Each engine gets its own ADR.
- **Non-SELECT paths** — INSERT / UPDATE / DELETE
  don't charge the budget yet. Mutation volume is
  already bounded by `StagingLimits`; the budget is
  about *read* cost.
- **Byte / iteration charges** — the SELECT path
  charges rows only. `max_bytes_materialized` and
  `max_iterations` remain available on the API but
  are unused until a future ADR plumbs them through
  projection + window code.
- **Cancellation** — breach surfaces at the next
  check point only; a tight inner loop without a
  checkpoint isn't interrupted mid-iteration.
- **Shared parent budget** — one budget per session.
  Parent/child plan-node budgets need `Arc`-shared
  trackers.

## 3. Compliance

New integration test file `history_sql_budget.rs`:

- SELECT within budget succeeds.
- SELECT over row budget errors with
  `SqlError::Budget(BudgetExceeded::Rows)`.
- SELECT past deadline errors with
  `SqlError::Budget(BudgetExceeded::Duration)`.
- `QueryBudget::unbounded()` never errors.
- Budget accumulates across statements in one session
  (first SELECT consumes, second breaches).
- Existing `with_session` path unchanged.
- 6 new integration tests.

## 4. Honest status

First engine wiring of the query budget. **BRD-RUNTIME-003
query-level timeout + resource bound** moves from
FOUNDATION ONLY to **PARTIAL**: one SELECT path in one
engine now consults the budget. The remaining engines,
mutation paths, byte charges and cancellation semantics
stay OPEN per §2.
