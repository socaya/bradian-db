# ADR 0229 — WHERE col IN (SELECT …)

## Status

Accepted.

## Context

List-valued predicates currently require a literal list.
Subquery-driven IN / NOT IN is the standard SQL idiom and
removes the caller-side two-roundtrip burden.

## Decision

Pre-process the SQL in `execute_impl` to resolve every
`IN (SELECT …)` / `NOT IN (SELECT …)` subquery before parsing
the outer statement.

- `find_in_subquery` walks the SQL respecting single-quoted
  strings and word boundaries, locating `IN (`-then-`SELECT`
  spans.
- `find_matching_paren` balances parens (also quote-aware) to
  find the subquery's closing `)`.
- `resolve_in_subqueries` runs each subquery through
  `execute_impl` (recursive, but each pass strips one
  subquery so the recursion terminates), validates the
  result has exactly one column, serializes the values as a
  literal list via `format_in_list`, and rewrites the SQL.
- Supported literal types: Int32, Int64, Decimal, Boolean,
  String, Json, NULL. Uuid / TimestampTz surface
  `SqlError::Type` (no safe literal form yet).
- Empty subquery → `IN (NULL)` which matches nothing (SQL
  NULL-safe semantics; the IN matcher drops NULL list members
  per ADR 0206).
- No correlated subqueries. The subquery runs independently
  of the outer row context.

## Consequences

- `UNION` splitting runs after subquery rewrite, so a
  subquery inside a UNION arm resolves normally.
- 6 cases in `tests/base_subquery_in.rs` cover IN selects
  matching rows, NOT IN excludes, empty subquery leaves no
  matches, text values, subquery with its own WHERE,
  multi-column-subquery rejection.
