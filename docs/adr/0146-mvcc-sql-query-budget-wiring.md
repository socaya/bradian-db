# ADR 0146 — MvccSqlEngine query-budget wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Fourth and final
SQL-engine integration of the ADR 0141 `BudgetTracker`,
completing the SELECT-path coverage across every current
SQL engine: `SqlEngine` (ADR 0144), `HistorySqlEngine`
(ADR 0143), `DomainSqlEngine` (ADR 0145), and
`MvccSqlEngine` (this ADR).

## 1. Scope

Three narrow edits in
`crates/bradian-relational/src/mvcc_sql.rs`:

1. **`MvccSqlEngine::with_session_budget(budget, cb)`**
   — new constructor method that opens an
   `MvccSqlSession` with a `BudgetTracker::new(budget)`.
2. **`MvccSqlSession.budget: Option<BudgetTracker>`** —
   new optional field; `None` preserves pre-ADR behavior
   (used by `with_session`).
3. **SELECT-path instrumentation** — the `"select"` arm
   of `execute_inner` calls `budget.check_deadline()`
   before scan, `budget.charge_rows(rows.len())` +
   `budget.check_deadline()` after materializing rows.

## 2. Out of scope (follow-ups)

- **Non-SELECT paths** — INSERT / UPDATE / DELETE do
  not charge the budget (mutation volume is bounded
  by `StagingLimits`).
- **Byte / iteration charges** — SELECT charges rows
  only. `max_bytes_materialized` and `max_iterations`
  remain reserved for future projection / window
  instrumentation.
- **Cancellation** — breach surfaces at the next
  check point only.
- **Shared parent budget** — one `BudgetTracker` per
  session.

## 3. Compliance

New integration test file `mvcc_sql_budget.rs`:

- SELECT within budget succeeds.
- SELECT over row budget → `SqlError::Budget(
  BudgetExceeded::Rows)`.
- SELECT past deadline → `SqlError::Budget(
  BudgetExceeded::Duration)`.
- `QueryBudget::unbounded()` never errors.
- Budget accumulates across two SELECTs in one session.
- Existing `with_session` path unchanged.
- INSERT does not charge the budget (0-row cap +
  successful INSERT).
- 7 new integration tests.

## 4. Honest status

**Fourth and final engine wiring**. **BRD-RUNTIME-003
query-level timeout + resource bound** moves from
PARTIAL to **COMPLETE for the SELECT path across every
SQL engine**. Non-SELECT charges, byte / iteration
charges, and cancellation semantics stay OPEN per §2.
