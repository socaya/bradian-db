# ADR 0144 — Base SqlEngine query-budget wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Second engine wiring
of the ADR 0141 `BudgetTracker`, following ADR 0143 for
`HistorySqlEngine`. Same honest-scope pattern: SELECT
path only, additive API, existing callers unchanged.

## 1. Scope

Two edits in `crates/bradian-relational/src/sql.rs`:

1. **`SqlEngine::execute_with_budget(sql, budget)`** —
   new public method. Takes `&mut BudgetTracker`. Routes
   through the shared `execute_impl(sql, &default,
   Some(budget))`.
2. **`execute_impl(sql, control, budget)`** — private
   helper the existing `execute_with_control` and the
   new `execute_with_budget` both call. The SELECT
   branch calls `budget.check_deadline()` once before
   the scan and `budget.charge_rows(all.len())` +
   `check_deadline()` after materializing rows.

`execute` / `execute_as` / `execute_with_control`
signatures and behavior unchanged — all route through
`execute_impl` with `budget = None`.

## 2. Out of scope (follow-ups)

- **Session-scoped budget** — the base engine has
  no multi-statement session like `HistorySqlEngine`.
  A future `with_sql_session_and_budget` can thread the
  tracker through a session if BRD-DRV-003 (prepared
  statement session) lands.
- **Non-SELECT paths** — INSERT / UPDATE / DELETE do
  not charge the budget (mutation volume is already
  bounded elsewhere). Non-SELECT charges are a
  follow-up.
- **Byte / iteration charges** — SELECT charges rows
  only. `max_bytes_materialized` and `max_iterations`
  remain reserved for future projection / window
  instrumentation.
- **Remaining engines** — `MvccSqlEngine` and
  `DomainSqlEngine` still OPEN.

## 3. Compliance

New integration test file `sql_engine_budget.rs`:

- SELECT within budget succeeds; row charge visible.
- SELECT over row budget returns
  `SqlError::Budget(BudgetExceeded::Rows)`.
- SELECT past deadline returns
  `SqlError::Budget(BudgetExceeded::Duration)`.
- Unbounded budget never errors; row counter still
  accumulates across calls.
- Budget accumulates across successive SELECTs.
- `execute()` path without budget behaves unchanged.
- INSERT does not charge the budget (0-row cap +
  successful INSERT).
- 7 new integration tests.

## 4. Honest status

Second engine wiring. **BRD-RUNTIME-003** remains
**PARTIAL** — two of four SQL engines consult the
budget now; `MvccSqlEngine` and `DomainSqlEngine` still
OPEN, non-SELECT charges still OPEN, byte/iteration
charges still OPEN.
