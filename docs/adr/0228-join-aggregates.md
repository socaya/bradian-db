# ADR 0228 — Aggregates + GROUP BY in joined queries

## Status

Accepted.

## Context

ADR 0227 produced composite rows from two-table joins but
restricted projections to bare qualified columns. Aggregations
over joined data (`SELECT u.name, SUM(o.total) … GROUP BY
u.name`) are the natural completeness extension.

## Decision

- Aggregate parsers accept a two-part `alias.col` argument via
  `parse_agg_column`. GROUP BY and HAVING key references share
  the same extension.
- A synthetic composite `Table` (via `build_composite_table`)
  names every column `alias.col`, so `locate` / `evaluate_aggregate`
  / `evaluate_grouped` work unchanged on composite rows.
- The joined SELECT path parses GROUP BY / HAVING against the
  composite schema and dispatches to a new
  `execute_join_grouped`:
  1. Nested-loop join builds composite rows (reusing ADR 0227's
     OUTER semantics).
  2. If any aggregate or GROUP BY is present, feed composite
     rows through `evaluate_grouped` with the synthetic table.
  3. Single-aggregate / no-GROUP-BY short-circuits via
     `evaluate_aggregate` just like the single-table scalar
     aggregate path.
- Rules preserved from earlier ADRs: non-aggregate projections
  must appear in the GROUP BY list; aggregates without GROUP BY
  must own the whole projection list.

## Consequences

- ORDER BY on grouped joined output still parses but is
  discarded (same rule as the single-table grouped path).
- JSON-path projections remain unsupported in joined queries.
- 9 cases in `tests/base_join_group_by.rs` cover COUNT per
  group, SUM per group, MIN/MAX per group, scalar COUNT(*) /
  SUM, HAVING, LEFT JOIN + COUNT(col) counting NULLs out,
  two-column GROUP BY, COUNT DISTINCT per group. The 10
  OUTER-join and 100+ single-table tests still pass.
