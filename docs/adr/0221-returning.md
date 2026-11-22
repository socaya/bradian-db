# ADR 0221 — RETURNING on INSERT / UPDATE / DELETE

## Status

Accepted.

## Context

DML today only reports an affected-row count. Callers that need
the actual inserted / updated / deleted rows have to follow with
a SELECT, which doubles the round-trip count and can observe
intermediate state when another session is active.

## Decision

Accept a trailing `RETURNING <projection-list> | *` clause on
every DML statement across all four engines. When present, the
statement returns the projected rows instead of a scalar
`Command { affected }`.

- `parse_returning(parser, table)` admits `RETURNING *` (expands
  to every column) or a comma-separated projection list. Each
  projection is resolved against the table schema via `locate`;
  unknown columns are rejected at parse time.
- `finish_dml_result_with_table(table, captured_rows, projections)`:
  - `projections.is_none()` → emits `Command { affected }` with
    the captured row count.
  - Otherwise projects each captured row against the full-schema
    layout and emits `Rows { columns, rows }`. Reuses the
    existing `apply_projection` so JSON-path projections work
    the same way as SELECT.
- Capture points:
  - `INSERT` captures each row **after** coercion and constraint
    checks, in VALUES-tuple order. Matches ADR 0220's batch
    semantics — a mid-batch failure leaves earlier rows in the
    captured list on the base engine (transactional engines
    roll the whole batch back together on COMMIT abort).
  - `UPDATE` captures each row **after** SET + JSON mutations
    and all validators. Zero-row UPDATE returns an empty row set.
  - `DELETE` captures each row **before** removal so RETURNING
    shows the deleted contents.

## Consequences

- `SqlResultSet::Rows { columns: ["id", "name"], rows: [[1,"x"]] }`
  is the new shape for DML with RETURNING. Callers that check
  `SqlResultSet::Command { .. }` without RETURNING are unaffected.
- `RETURNING col -> 'json_path'` works because it reuses
  `apply_projection`. GROUP BY / DISTINCT / aggregates are not
  valid in a RETURNING list (future ADR if needed).
- No conflict with ORDER BY / LIMIT on DML (those don't exist on
  DML in this engine).
- Validated by `tests/base_returning.rs` (12 cases: INSERT
  RETURNING *, subset, UPDATE RETURNING, DELETE RETURNING, no
  RETURNING still Command, unknown-column rejection, empty-match
  returns empty rows, multi-row INSERT RETURNING, Domain +
  MVCC parity). The 15 multi-row INSERT, 10 DISTINCT, 11 GROUP
  BY tests still pass.
