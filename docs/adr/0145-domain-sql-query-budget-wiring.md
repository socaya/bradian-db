# ADR 0145 — DomainSqlEngine query-budget wiring (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Third engine-level
integration of the ADR 0141 `BudgetTracker`, following
ADR 0143 (`HistorySqlEngine`) and ADR 0144 (`SqlEngine`).
Same honest-scope pattern: SELECT path only, additive
API, existing callers unchanged.

## 1. Scope

Three narrow edits in
`crates/bradian-relational/src/domain_sql.rs`:

1. **`DomainSqlEngine::with_session_budget(budget, cb)`**
   — new constructor method that opens a
   `DomainSqlSession` with a `BudgetTracker::new(budget)`.
2. **`DomainSqlSession.budget: Option<BudgetTracker>`** —
   new optional field. `None` preserves pre-ADR behavior
   exactly (used by `with_session`).
3. **SELECT-path instrumentation** — the `"select"` arm
   of `execute_inner` calls `budget.check_deadline()`
   before scan, then `budget.charge_rows(rows.len())` +
   `budget.check_deadline()` after materializing rows.

## 2. Out of scope (follow-ups)

- **MvccSqlEngine** — the fourth and last SQL engine
  still OPEN. A follow-up ADR mirrors this wiring.
- **Non-SELECT paths** — INSERT / UPDATE / DELETE do
  not charge the budget (mutation volume bounded via
  `StagingLimits`).
- **Byte / iteration charges** — rows only.
- **Cross-table SELECT budget semantics** — Domain
  sessions can span multiple tables per BEGIN. The
  budget accumulates across SELECTs in the session
  as expected, but per-table caps aren't modeled.

## 3. Compliance

New integration test file `domain_sql_budget.rs`:

- SELECT within budget succeeds.
- SELECT over row budget errors with
  `SqlError::Budget(BudgetExceeded::Rows)`.
- SELECT past deadline errors with
  `SqlError::Budget(BudgetExceeded::Duration)`.
- `QueryBudget::unbounded()` never errors.
- Budget accumulates across two SELECTs in one session.
- Existing `with_session` path unchanged.
- INSERT does not charge the budget (0-row cap +
  successful INSERT).
- 7 new integration tests.

## 4. Honest status

Third engine wiring. **BRD-RUNTIME-003** remains
PARTIAL: three of four SQL engines consult the
budget now. `MvccSqlEngine` still OPEN. Non-SELECT
charges, byte / iteration charges, cancellation
semantics still OPEN.
