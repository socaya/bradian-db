# ADR 0150 — Iteration-budget charges (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Activates ADR 0141
`QueryBudget::max_iterations` from a reserved knob to an
observable counter. One iteration = one executed
statement, giving operators a session-level cap
orthogonal to rows (ADR 0143-0147) and bytes (ADR 0148-
0149).

## 1. Scope

All four SQL engines charge `budget.charge_iteration()`
once at the top of each statement's execution path:

- `SqlEngine::execute_impl` — after the operation mutex
  guard.
- `HistorySqlSession::execute_inner` — after
  parse-command + failed-statement check.
- `DomainSqlSession::execute_inner` — after parse-command.
- `MvccSqlSession::execute_inner` — after parse-command.

A statement that fails before the parse (e.g.,
`failed_statement` rejection in `HistorySqlSession`) is
not counted; a parse-error that still reaches the
charge point is counted. BEGIN / COMMIT / ROLLBACK /
SAVEPOINT / DDL all count as iterations.

## 2. Out of scope (follow-ups)

- **Per-row inner-loop iterations** — would require
  per-engine `charge_iteration()` in projection /
  window code. The current meaning is simpler: one
  iteration per statement.
- **Mid-scan cancellation** — remains a check-point
  concern, not a wall-clock interrupt.
- **DDL sub-operation charges** — `CREATE TABLE`
  counts as one iteration today; a future ADR can
  decompose into column / constraint sub-charges.
- **Session-sharing** — one tracker per session; no
  parent/child budget tree.

## 3. Compliance

Updated four engine test files. Totals now:

- `sql_engine_budget.rs` — 20 tests (added 2:
  per-statement counter + breach-after-cap).
- `history_sql_budget.rs` — 11 tests (added 1:
  breach-after-cap with BEGIN counted).
- `domain_sql_budget.rs` — 11 tests (added 1:
  breach-after-cap with BEGIN counted).
- `mvcc_sql_budget.rs` — 11 tests (added 1:
  breach-after-cap with BEGIN counted).

Three successive `execute` calls on SqlEngine report
`iterations == 3`. A `max_iterations = 2` budget
admits two statements and breaches on the third with
`SqlError::Budget(BudgetExceeded::Iterations)`.

## 4. Honest status

**BRD-RUNTIME-003** now has row + byte + iteration
coverage on every DML / SELECT path across every SQL
engine. Mid-scan cancellation, DDL sub-operation
decomposition, multi-row INSERT, and pre-mutation
prediction remain OPEN per §2.
