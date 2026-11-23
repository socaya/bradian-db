# ADR 0222 — TRUNCATE RETURNING

## Status

Accepted.

## Context

ADR 0221 added `RETURNING` to INSERT / UPDATE / DELETE. `TRUNCATE`
is the natural fourth candidate — callers truncating a table to
replay inserts often want a snapshot of what was discarded.

## Decision

Accept a trailing `RETURNING <col[, col]> | *` on `TRUNCATE
TABLE <name>`. Captures each row **before** `store.delete`
removes it. Reuses `parse_returning` and
`finish_dml_result_with_table` from ADR 0221.

## Consequences

- Base engine only — the transactional engines don't expose
  `TRUNCATE`.
- 5 cases in `tests/base_truncate_returning.rs` cover
  no-RETURNING Command, RETURNING *, RETURNING subset,
  unknown-column rejection, and empty-table RETURNING.
