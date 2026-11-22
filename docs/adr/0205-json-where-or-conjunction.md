# ADR 0205 — OR / DNF combinators in JSON-path WHERE

## Status

Accepted.

## Context

ADR 0204 added AND-chained JSON-path predicates. Callers also need
disjunction — `body ->> 'owner' = 'bob' OR body ->> 'owner' =
'carol'` — and mixed AND/OR without parentheses.

## Decision

Model the JSON-path WHERE in disjunctive normal form (DNF): the
outer `Vec` is OR-joined, each inner `Vec` is an AND-group. A row
matches when at least one AND-group fully holds.

- `SelectWhere::JsonPathPredicates(Vec<Vec<JsonPredicate>>)` replaces
  the ADR 0204 flat shape. Single-predicate and AND-only queries
  remain one outer element.
- `parse_select_where` loops, consuming `AND` (same group) or
  `OR` (start a new group). Both must be followed by another
  JSON-path atom; a PK equality in that position is rejected.
- Precedence: `AND` binds tighter than `OR`, so `A AND B OR C AND D`
  parses as `(A AND B) OR (C AND D)` — no parentheses required or
  supported.
- `apply_where_filter` short-circuits at the first matching group;
  rows appearing in multiple groups are kept once (not duplicated).
  Column positions resolved once per group.

## Consequences

- Carries through all four engines (`SqlEngine`, `DomainSqlEngine`,
  `MvccSqlEngine`, `HistorySqlEngine`) for SELECT, UPDATE, DELETE
  without further wiring — they only matched on the outer variant.
- Scan behavior unchanged: still a full scan + per-row filter, with
  rows/bytes charged pre-filter.
- No OR followed by PK, no NOT, no parentheses. Those would require
  a full predicate tree; deliberately out of scope.
- Validated by `tests/json_path_where_or.rs` (12 cases: disjunction
  truth, deduplication, AND-tighter-than-OR, three OR groups,
  container-form, DNF mixing, UPDATE / DELETE, PK-after-OR
  rejection, Domain + MVCC parity). Existing ADR 0204 AND tests
  continue to pass (11 cases).
