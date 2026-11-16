# ADR 0135 — SET NOT NULL backfill validator (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch B B3 primitive.
B1 (DEFAULT expressions parser) remains the only Batch B
item still OPEN.

## 1. Scope

New module `bradian_relational::backfill_validator`:

- `plan_backfill(columns, rows, target_column,
  fill_value) -> Result<BackfillPlan>` — pure function.
- `BackfillPlan { column, column_index, fill_value,
  null_row_indices }` — executor rewrites exactly the
  rows at `null_row_indices` with `fill_value`.
- `null_count()` / `is_noop()` helpers.
- `BackfillError { ColumnNotFound / AlreadyNotNull /
  FillIsNull / FillTypeMismatch / RowWidthMismatch }`.

Behavior:
1. Reject unknown column.
2. Reject if column is already NOT NULL (no-op DDL).
3. Reject `Value::Null` as fill (defeats the purpose).
4. Validate fill against declared `DataType` using the
   same permissive matcher as ADR 0124 defaults.
5. Scan rows, record every index where the target column
   is `Value::Null`.
6. Return the plan; empty `null_row_indices` means
   backfill is a no-op.

## 2. Out of scope (follow-ups)

- **Actual row mutation** — the plan is a pre-flight.
  A future executor ADR applies it under the RowStore
  mutation lock.
- **`Catalog::set_not_null(id, column, fill)` mutator**
  — needs ADR 0126 dependency check (SET NOT NULL on
  FK-referenced column has cascade implications), WAL
  journaling, and HistorySqlEngine schema
  reconciliation.
- **Online / concurrent backfill** — current design is
  single-threaded under the mutation lock.
- **DEFAULT expression as fill value** — only literal
  values accepted. `CURRENT_TIMESTAMP` etc. are B1.
- **Rollback of a partially applied backfill** — the
  executor must choose between atomic batch commit
  (preferred) and resumable mid-batch WAL records.

## 3. Compliance

- `null_row_indices` lists only rows where the target
  column was NULL.
- No-null-row case yields `is_noop() == true`.
- Empty row list yields `is_noop() == true`.
- Unknown column → `ColumnNotFound`.
- Already-NOT-NULL column → `AlreadyNotNull`.
- Null fill → `FillIsNull`.
- Type mismatch → `FillTypeMismatch` (type-level +
  Varchar length bound + JSON content).
- Row width mismatch → `RowWidthMismatch`.
- Fill value is preserved exactly in the plan.
- 11 new tests in `backfill_validator::tests`.

## 4. Honest status

Foundation only. **BRD-DDL-005 SET NOT NULL with
online backfill** is not yet satisfied because the
executor, catalog mutator, WAL journal, and online
coordination all remain OPEN. The validator ensures a
future DDL coordinator can pre-check the request
before taking any lock.
