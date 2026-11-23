# ADR 0224 — INSERT INTO t (cols) SELECT …

## Status

Accepted.

## Context

Copying rows between tables currently requires reading via
SELECT and re-inserting via multi-row INSERT in the caller.
SQL's `INSERT INTO … SELECT` is the idiomatic shape.

## Decision

Accept a narrow MVP subset of `INSERT INTO t (cols)
SELECT cols FROM src [WHERE …] [RETURNING …]`. Executed inline
in the base `SqlEngine` INSERT branch.

- Projection list is bare columns only (no JSON paths, no
  aggregates, no DISTINCT, no GROUP BY). The source column
  count must match the target column list.
- `WHERE` reuses `parse_select_where` (PK equality or JSON path
  predicate), enabling filtered copies.
- Each source row is projected into the target column order,
  coerced through `check_row` / `foreign_key_check` /
  `UniqueConstraintChecker`, then inserted. Cross-type numeric
  widen `Int32 ↔ Int64` so narrower source columns feed wider
  targets.
- `RETURNING` composes via the same `finish_dml_result_with_table`
  path.

## Consequences

- No `INSERT … SELECT` from a joined / aggregated source yet.
- No `VALUES` + `SELECT` mixing in one statement.
- Base `SqlEngine` only — transactional engines keep the
  VALUES-only shape until a shared SELECT-execution helper
  lands.
- 7 cases in `tests/base_insert_select.rs` cover copy-all,
  WHERE filter, column-count mismatch rejection, RETURNING,
  empty source, Int32 → Int64 widen, duplicate PK rejection.
