# ADR 0151 — Pre-mutation budget preflight (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Fixes the semantic
awkwardness in the base `SqlEngine`: before this ADR a
budget breach on INSERT / UPDATE / DELETE returned
`SqlError::Budget` **after** the storage write had
already succeeded, leaving the row committed. The
session-based engines (`HistorySqlEngine`,
`DomainSqlEngine`, `MvccSqlEngine`) have a transaction
wrapper that drops staged writes on error, so this ADR
targets the base engine only.

## 1. Scope

New method `BudgetTracker::preview_charge(rows, bytes)`:
- Checks `check_deadline()` + would-breach on rows +
  would-breach on bytes.
- Returns the matching `BudgetExceeded` variant without
  mutating any counter.
- Used by the base `SqlEngine` immediately before each
  storage mutation.

Wired in `sql.rs` `execute_impl`:
- INSERT — preview `(1, row_bytes(&values))` before
  `.insert(&values)`.
- UPDATE — preview `(1, row_bytes(&row))` before
  `.replace(&row)` (only when the WHERE key hits).
- DELETE — preview `(1, row_bytes(existing))` before
  `.delete(&pk)` (only when the row exists).
- TRUNCATE — preview the full scanned `(rows.len,
  rows_bytes)` before entering the delete loop.

The commit charge still happens after success, so the
tracker's counters reflect what actually landed.

## 2. Out of scope (follow-ups)

- **Session engines** — the three transaction-backed
  engines keep the post-mutation charge pattern.
  Breach there aborts the transaction, which drops
  staged writes; semantically equivalent to preflight.
- **Multi-row INSERT** — not yet supported by the
  parser; when it lands, preview will size-up the
  whole batch.
- **Partial TRUNCATE on preflight failure** — the
  full-set preview makes TRUNCATE all-or-nothing from
  the caller's view, matching the SQL expectation.
- **Mid-scan cancellation** — remains a check-point
  concern.

## 3. Compliance

Unit tests for the primitive (`query_budget::tests`):
- `preview_charge` does not mutate on success.
- Rejects over-row budget without mutation.
- Rejects over-byte budget without mutation.
- Preview-then-commit accumulates correctly.

Integration tests in `sql_engine_budget.rs`:
- `insert_over_byte_budget_is_rejected_before_storage_side_effect`
  — after the breach, follow-up SELECT shows the row
  was NOT inserted.
- `delete_over_byte_budget_is_rejected_before_storage_side_effect`
  — after the breach, the row still exists.

`sql_engine_budget.rs` total now 22 tests;
`query_budget::tests` total +4.

## 4. Honest status

**BRD-RUNTIME-003** gains the "transactional semantics
on the non-transactional engine" property. Mid-scan
cancellation and DDL sub-operation decomposition remain
OPEN.
