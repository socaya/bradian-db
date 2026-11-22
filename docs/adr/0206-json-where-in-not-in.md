# ADR 0206 — IN / NOT IN on JSON path WHERE

## Status

Accepted.

## Context

ADRs 0204–0205 added AND / OR DNF combinators on JSON-path WHERE,
but the only atomic predicate shape was `<col-path> <op> <literal>`.
Membership queries (`owner IN ('alice','bob')`) had to be unrolled
into a flat OR chain by the caller.

## Decision

Make the atomic predicate an enum carrying either a scalar
comparison or a list membership test.

- `JsonPredicate { projection, matcher: JsonMatcher }` replaces the
  old `{ projection, op, literal }` shape.
- `JsonMatcher::{ Compare { op, literal }, In { literals }, NotIn
  { literals } }`.
- Parser: after a JSON path LHS, try `IN (…)` and `NOT IN (…)`
  before falling through to the scalar `op literal` dispatch. `NOT`
  must be immediately followed by `IN`; standalone `NOT` is
  rejected. Empty `IN ()` is rejected at parse. Each literal is
  coerced using the same `as_text` rule as a scalar comparison.
- Evaluation preserves three-valued logic: a `NULL` extracted
  value fails the predicate; `NULL` list members are skipped on
  `IN` and fail-the-row on `NOT IN`.
- Call sites in `apply_where_filter` dispatch on `JsonMatcher`;
  AND and OR combinators from ADRs 0204/0205 compose unchanged.

## Consequences

- Carries through all four engines (`SqlEngine`, `DomainSqlEngine`,
  `MvccSqlEngine`, `HistorySqlEngine`) for SELECT, UPDATE, DELETE
  with no engine-side changes — the shape lives inside
  `apply_where_filter`.
- Scan cost unchanged: still a full scan + per-row filter.
- No subquery `IN (SELECT …)`, no ANY/ALL, no BETWEEN, no LIKE —
  future extensions.
- Validated by `tests/json_path_where_in.rs` (14 cases: text / numeric
  / single-value, NOT IN exclude, container form, AND composition,
  OR composition, UPDATE, DELETE, empty-list rejection, NOT-without-IN
  rejection, mixed IN+NOT IN DNF, Domain + MVCC parity). The 11
  AND, 12 OR, and 10 cmp tests still pass with the refactored
  matcher.
