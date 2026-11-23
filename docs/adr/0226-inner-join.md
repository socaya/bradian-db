# ADR 0226 — Simple INNER JOIN

## Status

Accepted.

## Context

Every SQL path so far is single-table. Combining rows from
related tables by a shared key requires JOIN — the biggest
remaining SQL shape.

## Decision

Accept a tightly-scoped two-table INNER JOIN in the base engine:

```
SELECT [table.col[, table.col]* | *]
FROM <qualified_t1>
JOIN  <qualified_t2> ON t1.c = t2.c
[WHERE t1_or_t2.col = lit]
```

- `parse_projection` accepts a two-part `table.col` reference
  (stored as a single `Projection::Column(String)` containing a
  dot). The single-table SELECT path rejects dotted projections
  explicitly.
- After `FROM` resolves the left table, the SELECT branch
  peeks for `JOIN`; if present it forks to `execute_inner_join`
  and returns, bypassing the single-table pipeline.
- `join_resolve_on` accepts either order in the ON equality
  and rejects references that don't name the join's tables or
  that are not two-part.
- `join_resolve_column` resolves a WHERE column reference to
  `(is_right_side, column_position)`.
- Execution is a nested-loop join: scan left, scan right, keep
  rows where the ON columns compare equal, optionally filter
  by the WHERE scalar predicate, project columns in the
  SELECT list order, label output as `table.col`.
- `SELECT *` concatenates left columns followed by right
  columns into the composite row.
- No aliases, no LEFT / RIGHT / FULL / CROSS, no USING, no
  multi-table JOINs, no aggregates / DISTINCT / GROUP BY /
  HAVING / ORDER BY / LIMIT in joined queries — each a future
  ADR.
- Base `SqlEngine` only; transactional engines retain the
  single-table shape.

## Consequences

- Admission charges both left and right row/byte totals before
  the nested-loop join runs.
- Composite row widths are bounded by the sum of participant
  table widths; result cardinality is bounded by
  `|left| × |right|`.
- Validated by `tests/base_inner_join.rs` (11 cases: basic
  join, bare-projection rejection, empty right, WHERE on left
  / on right, ON order reversed, left-only projection, unknown
  table rejection, `SELECT *` wildcard concatenation, no-match
  empty result, dotted-projection-without-JOIN rejection). All
  100 earlier single-table SELECT / DML tests still pass.
