# ADR 0227 — LEFT / RIGHT / FULL OUTER JOIN + table aliases

## Status

Accepted.

## Context

ADR 0226 shipped INNER JOIN with required table-name qualification.
Aliases and outer joins are the natural completeness extension.

## Decision

- `JoinKind::{Inner, LeftOuter, RightOuter, FullOuter}` controls
  the nested-loop behavior: inner emits match pairs only; left /
  right emit orphans from the respective side with NULL columns
  on the absent side; full emits both.
- Parser accepts `[LEFT|RIGHT|FULL] [OUTER]` prefix before
  `JOIN`, plus the explicit `INNER JOIN`.
- Table aliases: `FROM t [AS] alias JOIN t2 [AS] alias ON …`.
  Qualified refs inside `ON` / `SELECT` / `WHERE` resolve against
  the bound alias; when alias is bound, the raw table name is
  no longer a valid reference.
- `join_resolve_on` / `join_resolve_column` take explicit alias
  arguments and look up columns by table + alias.
- WHERE filter on OUTER joins treats NULLs on the orphan side
  as non-matches (so `WHERE orders.total = 100` on a LEFT JOIN
  drops the orphan left rows), matching SQL's WHERE-after-ON
  semantics.

## Consequences

- No multi-table JOINs, no CROSS JOIN, no USING clause.
- Base engine only; transactional engines retain the single-
  table SELECT.
- 10 cases in `tests/base_outer_join.rs` cover LEFT keeps
  orphans, RIGHT keeps orphans, FULL both sides, OUTER keyword
  optional, aliases with and without AS, explicit INNER
  keyword, empty-right LEFT, OUTER + WHERE on right drops
  orphans, alias rejects bare table name. The 11 ADR 0226
  INNER tests still pass.
