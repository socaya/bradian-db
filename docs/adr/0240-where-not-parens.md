# ADR 0240 — NOT prefix + parentheses in JSON-path WHERE

## Status

Accepted.

## Context

The ADR 0205 WHERE was restricted to disjunctive normal form
(OR-of-AND groups). Standard SQL needs arbitrary boolean trees:
`WHERE NOT (predicate)`, `WHERE (a OR b) AND c`. Second
meaningful P0-3 SQL-conformance slice.

## Decision

- `WhereTree` boolean tree with `Predicate`, `And(l, r)`,
  `Or(l, r)`, `Not(inner)` variants.
- `SelectWhere::JsonPathPredicates(WhereTree)` replaces the
  previous `Vec<Vec<JsonPredicate>>` DNF.
- `parse_where_expr` runs a four-tier precedence descent:
  OR (lowest) → AND → NOT (unary) → primary. Primary is
  either `(expr)` or a single predicate parsed by
  `parse_json_predicate` (which reuses every matcher from
  ADRs 0206–0210).
- `eval_where_tree` walks recursively with AND / OR short-
  circuiting and NOT truth flip. `eval_predicate` extracted
  the per-predicate three-valued-logic rules unchanged from
  ADR 0205.
- `apply_where_filter` takes `&WhereTree`; every engine's
  call site compiles unchanged because the pattern
  `SelectWhere::JsonPathPredicates(preds)` now binds `preds`
  to `&WhereTree`.

## Consequences

- Prior DNF-only queries still parse: `A AND B OR C` becomes
  `Or(And(A, B), C)` with standard AND-tighter-than-OR
  precedence. The 126 earlier WHERE tests (10 files) all pass
  unchanged.
- `NOT NOT P` double-cancels correctly.
- `NOT (a BETWEEN x AND y)` and `NOT a IN (...)` evaluate
  identically to the pre-existing `NOT BETWEEN` / `NOT IN`
  variants.
- Unbalanced parens yield `SqlError::Syntax`.
- JSON-path arithmetic infix on the WHERE side still rejected
  (future ADR).
- 9 new tests in `tests/json_path_where_not.rs`.
