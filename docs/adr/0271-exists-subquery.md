# ADR 0271 — EXISTS (SELECT …) subquery

## Status

Accepted.

## Context

The autonomous G1 run marked EXISTS as deferred because
pre-parsing inside the WHERE tree couldn't reach the engine
handle, and injecting a tautology at the SQL text level
needed something like `WHERE TRUE` which didn't parse. This
ADR closes both gaps.

## Decision

- `WhereTree::Bool(bool)` variant added. `parse_where_primary`
  consumes `TRUE` / `FALSE`. `eval_where_tree` returns the
  constant.
- `parse_select_where` routes `WHERE TRUE` / `WHERE FALSE`
  (and `WHERE NOT TRUE`, `WHERE (TRUE AND …)`, etc.) to the
  tree parser.
- New `resolve_exists_subqueries` pass runs after
  `resolve_in_subqueries` in `execute_impl`. For each
  top-level `EXISTS (SELECT …)` it executes the inner and
  substitutes the span with `TRUE` (non-empty) or `FALSE`
  (empty). `NOT EXISTS`, `EXISTS AND other`, `EXISTS OR
  other` all compose via the tree parser.
- New `find_exists_subquery(sql, from)` locator — mirrors
  `find_in_subquery` but scans for the `EXISTS` keyword at
  paren depth 0.

## Consequences

- Non-correlated EXISTS only. A correlated subquery (one
  that references outer columns) would need per-row
  re-execution, which this slice does not do.
- Composes freely with AND / OR / NOT around EXISTS.
- Empty EXISTS does NOT short-circuit the outer; it substitutes
  FALSE and the outer scan still runs (with no matching rows).
  A future slice could add a true short-circuit for
  cost savings.
- 7 cases in `tests/exists_subquery.rs` cover: inner
  non-empty → all; inner empty → none; WHERE TRUE; WHERE
  FALSE; EXISTS AND predicate; EXISTS OR predicate; NOT
  EXISTS.
