# ADR 0147 — Non-SELECT query-budget charges (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Extends ADR 0143 /
0144 / 0145 / 0146 (SELECT-only wiring across all four
SQL engines) to charge the ADR 0141 `BudgetTracker` for
mutations as well. Same additive-API pattern: callers
without a budget see no behavior change.

## 1. Scope

Four engines, five statement kinds now charge the budget:

| Engine | INSERT | UPDATE | DELETE | TRUNCATE |
|---|---|---|---|---|
| `SqlEngine` | ✅ +1 | ✅ +affected | ✅ +affected | ✅ +rows |
| `HistorySqlEngine` | ✅ +1 | ✅ +affected | ✅ +affected | n/a |
| `DomainSqlEngine` | ✅ +1 | ✅ +affected | ✅ +affected | n/a |
| `MvccSqlEngine` | ✅ +1 | ✅ +affected | ✅ +affected | n/a |

Instrumentation pattern per mutation path:
1. `budget.check_deadline()?` **before** the mutation.
2. Compute `affected` (0 or 1 for key-targeted
   UPDATE/DELETE; full row count for TRUNCATE).
3. `budget.charge_rows(affected)?` **after** the
   mutation succeeded. A missing row (`WHERE id =
   does-not-exist`) charges 0.

INSERT always charges 1 because the current parser
accepts a single row literal per statement.

## 2. Out of scope (follow-ups)

- **Multi-row INSERT** — current parser is one-row-at-a-
  time. When multi-row lands, the charge becomes the
  value count.
- **Byte charges** — `max_bytes_materialized` still
  unused. Charging materialized row *size* needs a
  per-row byte count; follow-up.
- **Iteration charges** — reserved for future projection
  / window code.
- **Cancellation** — breach detection remains at check
  points only; a tight inner loop without a checkpoint
  is not interrupted.
- **DDL charges** — CREATE / DROP / ALTER don't consult
  the budget. Catalog-level resource bounds are a
  separate ADR 0114 story.
- **BRD-RUNTIME-003 completion** — this ADR covers
  *all* DML paths across all four SQL engines but
  leaves byte / iteration / cancellation / DDL
  charges open. Terminal BRD compliance depends on
  those.

## 3. Compliance

Four updated test files; **35 total integration tests**:

- `sql_engine_budget.rs` — 11 tests. INSERT charges 1,
  UPDATE charges affected (hit + miss), DELETE charges
  affected, TRUNCATE charges every row removed.
- `history_sql_budget.rs` — 8 tests. UPDATE/DELETE
  charge affected count; miss charges 0.
- `domain_sql_budget.rs` — 8 tests. INSERT + UPDATE +
  DELETE in a single session accumulate correctly.
- `mvcc_sql_budget.rs` — 8 tests. Same shape as domain.

Behavior preserved: no-budget callers (`with_session`
/ `execute` / `execute_with_control`) see zero change.

## 4. Honest status

**BRD-RUNTIME-003** now has full DML coverage across all
four SQL engines. Byte charges, iteration charges,
cancellation semantics, DDL charges, and multi-row
INSERT remain OPEN — each is a narrow follow-up ADR
of the same shape.
