# ADR 0152 — Budget cancellation handle (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-17.** Adds cooperative
cancellation to the ADR 0141 `BudgetTracker` so a caller
can abort a running session from another thread. Mirrors
the existing `ScanControl::cancel()` pattern (ADR 0041)
but lives on the query-budget side, where it composes
with the full row / byte / iteration / deadline
coverage.

## 1. Scope

New types in `crates/bradian-relational/src/query_budget.rs`:

- `CancellationHandle` — `Arc<AtomicBool>` wrapper with
  `new()` / `cancel()` / `is_cancelled()`. Clone-able;
  all clones share one flag.
- `BudgetTracker::with_cancellation(handle)` — builder-
  style setter attaching a handle.
- `BudgetExceeded::Cancelled` — new error variant.

Wiring:
- `BudgetTracker::check_deadline()` now checks the
  cancellation flag before the duration cap.
- `preview_charge` → `check_deadline` → covers both.
- `charge_rows` / `charge_bytes` / `charge_iteration`
  intentionally do NOT check the flag — the engines
  already call `check_deadline` at every statement
  boundary (ADR 0150 iteration + explicit SELECT /
  DML deadline checks), so cancellation surfaces at
  the next check point.

New session constructors on three session-based engines
(`HistorySqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`):
- `with_session_budget_and_cancellation(budget, handle,
  cb)` — same shape as `with_session_budget` plus a
  shared handle.
- Base `SqlEngine::execute_with_budget` already takes a
  `&mut BudgetTracker`, so callers chain
  `.with_cancellation(handle)` on construction.

## 2. Out of scope (follow-ups)

- **Mid-row cancellation** — a running inner loop
  without a check point does not interrupt. The scan
  admission (`ScanControl` / `ResourceError::Cancelled`
  from ADR 0041) handles the scan side.
- **Timeout-driven cancellation** — operators who want
  "cancel after 5 seconds" can spawn a watcher thread
  that calls `handle.cancel()`.
- **Hierarchical cancellation** — one flag per
  handle; child trackers can share by cloning.

## 3. Compliance

Unit tests for the primitive (`query_budget::tests`):
- `CancellationHandle` defaults to not cancelled.
- `cancel()` flips the flag.
- Clones share the flag.
- `check_deadline` fires when cancelled.
- `preview_charge` fires when cancelled.

Integration tests:
- `sql_engine_budget.rs` —
  `cancellation_handle_aborts_next_statement`: flip
  mid-session, next SELECT returns `Cancelled`.
- `history_sql_budget.rs` —
  `cancellation_from_another_thread_aborts_next_statement`:
  flip mid-transaction, next SELECT returns
  `Cancelled`.

## 4. Honest status

**BRD-RUNTIME-003** now has duration + rows + bytes +
iterations + cooperative cancellation. Mid-row
cancellation and DDL sub-operation decomposition
remain OPEN.
