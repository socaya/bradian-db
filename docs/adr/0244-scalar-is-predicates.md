# ADR 0244 — Bare-column scalar IS predicates

## Status

Accepted.

## Context

Master-SRS P0-3 flags SQL conformance gaps. Before this ADR
the WHERE grammar routed bare-column references
(`WHERE col ...`) through the primary-key equality path only,
and `parse_json_predicate` explicitly rejected
`Projection::Column`. That meant `WHERE nullable_col IS NULL`
on a non-JSON column could not be written at all — a huge
usability hole that blocked ordinary `IS [NOT] NULL` and the
new `IS [NOT] DISTINCT FROM` (ADR 0243) on scalar columns.

## Decision

- `parse_select_where` dispatches to the boolean-tree parser
  when a bare column is followed by the `is` keyword, in
  addition to the existing JSON-arrow dispatch.
- `parse_json_predicate` accepts `Projection::Column`, but
  only through the IS branch. Other matchers
  (LIKE / ILIKE / IN / BETWEEN / `=`-style compare) on a bare
  non-JSON column return `SqlError::Unsupported` — they stay
  JSON-path-only for now.
- Literal coercion for `IS [NOT] DISTINCT FROM` on a bare
  column goes through `coerce(lit, column)` so the comparand
  takes the column's declared type (TEXT, INT64, DECIMAL,
  etc.). JSON-path predicates keep `coerce_where_literal`
  with the existing `->` / `->>` as-text rules.

## Consequences

- Only `IS [NOT] NULL` and `IS [NOT] DISTINCT FROM` work on
  bare scalar columns. Other bare-column predicates
  (`LIKE`, `IN`, `BETWEEN`, scalar `=` / `!=` / `<` / `<=`
  beyond PK equality) remain unsupported — each is a separate
  P0-3 slice.
- Type mismatches on bare-column IS DISTINCT FROM surface as
  `SqlError::Type` (through `coerce`), which is the same
  shape existing INSERT type errors use.
- 9 cases in `tests/scalar_is_null.rs` cover: IS NULL,
  IS NOT NULL, IS DISTINCT FROM matching nulls and non-equal
  rows, IS NOT DISTINCT FROM matching only equal rows, NOT
  wrapping, OR composition with IS NULL, LIKE still rejected,
  type mismatch rejected, IS NULL on a NOT NULL PK column
  returns no rows.
