# ADR 0204 — AND-chained JSON-path predicates in WHERE

## Status

Accepted.

## Context

ADR 0190 introduced a JSON-path WHERE branch that accepted a
single `col -> seg = literal` form; ADR 0195 extended the operator
set to `=, <>, <, <=, >, >=`. Callers often need to match a row on
two or more JSON criteria at once, e.g. `body ->> 'status' =
'open' AND body -> 'age' > 40`. Previously only PK predicates
supported `AND`, and the two WHERE shapes could not be mixed.

## Decision

Extend the JSON-path WHERE branch to admit one-or-more predicates
joined by `AND`. All predicates must be JSON-path shaped; mixing
with a PK equality is rejected.

- `JsonPredicate { projection, op, literal }` bundles one atom.
- `SelectWhere::JsonPathPredicates(Vec<JsonPredicate>)` replaces
  the old single-atom `JsonPathPredicate { … }` variant.
- `parse_select_where` loops: after parsing an atom, if the next
  token is `AND`, the following column name must be followed by
  `->`, `->>`, `#>`, or `#>>`. Otherwise it raises
  `SqlError::Unsupported("AND after a JSON-path predicate must be
  another JSON-path predicate")`. This preserves PK-only `AND` on
  the PK branch and JSON-only `AND` on the JSON branch, with no
  cross-mixing.
- `apply_where_filter` now takes `&[JsonPredicate]` and keeps a
  row only when every predicate holds. Early-exit on first miss;
  three-valued NULL semantics are preserved per predicate.
- PK equality grammar is unchanged.

## Consequences

- Four engines (`SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`,
  `HistorySqlEngine`) now route multi-predicate JSON WHERE on
  SELECT / UPDATE / DELETE.
- Scan behavior is unchanged: a JSON-path WHERE still triggers a
  full scan + per-row filter. Admission charges stay the same
  (rows + bytes pre-filter).
- No OR, NOT, parentheses, or cross-column predicates — out of
  scope.
- Validated by `tests/json_path_where_and.rs` (11 cases: AND
  conjunctive truth, operator mix, container-form chain, UPDATE
  / DELETE application, rejection of PK suffix, and per-engine
  parity for Domain + MVCC).
