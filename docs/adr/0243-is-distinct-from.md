# ADR 0243 — IS [NOT] DISTINCT FROM on JSON-path WHERE

## Status

Accepted.

## Context

Master-SRS P0-3 flags SQL conformance gaps that block the
Bradian SQL surface from being self-contained. `IS NULL` and
`IS NOT NULL` already shipped in ADR 0207, but their
NULL-aware siblings `IS DISTINCT FROM` and `IS NOT DISTINCT
FROM` were still unsupported. Those operators are how SQL
expresses "unequal, even if one side is NULL" and "equal,
including when both are NULL" without the three-valued-logic
trap that `=` / `!=` fall into on NULLs.

## Decision

- Two new `JsonMatcher` variants:
  `IsDistinctFrom { literal }` and
  `IsNotDistinctFrom { literal }`.
- Parser: the `IS` branch of `parse_json_predicate` now
  dispatches on the next word. `NULL` (optionally preceded by
  `NOT`) keeps its existing semantics. `DISTINCT FROM`
  (optionally preceded by `NOT`) reads one literal. Anything
  else returns `SqlError::Unsupported`.
- Evaluator: both new matchers bypass the global NULL guard
  in `eval_predicate` so they receive the raw extracted value.
  Truth table:
  - both NULL → not distinct (`IS DISTINCT FROM` → false;
    `IS NOT DISTINCT FROM` → true).
  - one NULL, one non-NULL → distinct.
  - both non-NULL → ordinary `compare_values_for_check` equality.

## Consequences

- Scoped to JSON-path WHERE predicates. Bare-column scalar
  WHERE beyond primary-key equality (including `IS NULL` on
  non-JSON columns) is a separate conformance hole and
  remains open.
- NULL literal in the RHS is not reachable through the base
  `literal()` grammar — a future ADR adds `NULL` as a literal
  if the surrounding parser accepts it.
- 9 cases in `tests/json_path_where_is_distinct.rs` cover:
  distinct-from-non-null-matches-non-equal-and-null,
  not-distinct-from-non-null-only-matches-equal,
  distinct-in-AND, distinct-with-`->`-terminator,
  missing FROM rejected, `IS NOT <lit>` without
  `DISTINCT FROM` rejected, distinct-with-OR composition,
  NOT wrapping, absent-key rows under `IS NOT DISTINCT FROM`.
