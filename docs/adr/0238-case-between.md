# ADR 0238 — BETWEEN in CASE WHEN predicates

## Status

Accepted.

## Context

ADR 0233's CASE predicate is `col op lit`. Numeric range
classification (`CASE WHEN score BETWEEN 70 AND 100 THEN
'high' …`) is a common SQL idiom that the compare-only shape
couldn't express concisely.

## Decision

Replace `CaseArm { column, op, literal, result }` with
`CaseArm { predicate: CasePredicate, result }` where
`CasePredicate` is:

- `Compare { column, op, literal }` — unchanged shape.
- `Between { column, low, high }` — inclusive range (SQL-99).
- `NotBetween { column, low, high }` — inverse.

Parser detects `BETWEEN` / `NOT BETWEEN` after the column
reference and parses `low AND high`.

Evaluator short-circuits when any side of the range is NULL
(matches ADR 0208's scalar WHERE BETWEEN semantics).

## Consequences

- No AND/OR in CASE predicates yet — the range form covers
  the most-requested use; arbitrary boolean composition is a
  future ADR.
- Existing CASE tests from ADR 0233 remain green under the
  new `CasePredicate::Compare` variant.
