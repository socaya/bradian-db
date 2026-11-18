# ADR 0156 — Budget + policy combined constructors (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Callers who need
both ADR 0141 resource-bounding and ADR 0123 row/
column/tenant filtering in the same session now have a
single constructor instead of having to choose one.

## 1. Scope

New constructors on each SQL engine:

- `SqlEngine::execute_with_budget_and_policy(sql,
  &mut BudgetTracker, &TableAccessPolicy,
  &AuthorizationContext)`.
- `HistorySqlEngine::with_session_budget_and_policy(
  budget, policy, authz, cb)`.
- `DomainSqlEngine::with_session_budget_and_policy(…)`.
- `MvccSqlEngine::with_session_budget_and_policy(…)`.

Each routes through the existing shared execution
helper with both the budget tracker and policy fields
populated. No new SELECT-path logic — ADR 0144/0153
(base) and ADR 0146/0154 (sessions) already consult
`budget` + `policy` independently.

**Charge-before-filter ordering:** the SELECT path
charges the budget against the *pre-filter* row count.
This matches operator intuition — "scanning 10 rows
cost 10 rows of budget even if the policy hid 8" — and
preserves the pre-ADR charge semantics exactly.

## 2. Out of scope (follow-ups)

- **Charge-after-filter mode** — some operators may
  want "I was billed for 2 rows because that's what I
  saw." A future ADR can offer a mode flag on
  `QueryBudget`.
- **Cancellation handle in combined constructor** —
  callers who want all three (budget + cancellation +
  policy) can wire via the existing tracker builder
  `.with_cancellation(h)` before passing it to
  `execute_with_budget_and_policy`.

## 3. Compliance

New test file `sql_engine_budget_and_policy.rs`
(4 integration tests):

- `sql_engine_budget_plus_policy_filters_and_charges`:
  rows filtered by policy, but budget charged pre-
  filter (3 scanned, 2 returned).
- `sql_engine_budget_breach_surfaces_despite_policy`:
  tight row cap breaches even when the policy would
  have filtered.
- `domain_engine_budget_plus_policy_combined`:
  session-engine happy path.
- `mvcc_engine_budget_plus_policy_combined`:
  session-engine happy path.

## 4. Honest status

**BRD-SEC-002** + **BRD-RUNTIME-003** compose cleanly on
every SQL engine. Catalog storage of policies
(ADR 0157) is the remaining prerequisite for a
turn-key RLS/CLS/tenant experience.
