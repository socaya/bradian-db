# ADR 0218 — HAVING on grouped SELECT

## Status

Accepted.

## Context

ADR 0217 added `GROUP BY` with mixed bare-column and aggregate
projections but had no way to filter the resulting buckets by
their aggregate value. The SQL-standard shape is `HAVING
<predicate>` after `GROUP BY`.

## Decision

Add a single-comparison `HAVING` clause that runs per bucket
inside `evaluate_grouped`.

- `HavingClause { term, op, literal }` + `HavingTerm::{ GroupKey,
  Aggregate }`.
- `parse_having(p, table, group_keys)` runs after
  `parse_group_by`. HAVING without a GROUP BY clause is rejected
  with `SqlError::Unsupported`. The left-hand term is either an
  aggregate call (recognized by `consume_aggregate` so it mirrors
  the projection grammar — `COUNT(*)`, `MIN(col)`, `MAX(col)`,
  `SUM(col)`, `AVG(col)`, `COUNT(col)`, `COUNT(DISTINCT col)`) or
  a bare column that must appear in `group_keys`. Non-group-key
  columns on the left are rejected.
- Operator is any scalar comparison admitted by
  `word_or_symbol_pair_for_check` (`=`, `<>`, `<`, `<=`, `>`,
  `>=`, matching ADR 0195's scalar WHERE).
- Evaluation inside `evaluate_grouped` reuses
  `evaluate_aggregate` for the Aggregate term and reads the
  bucket's key tuple for the GroupKey term. The literal is
  coerced with the same cross-type rule
  (`compare_values_for_check`) used by scalar WHERE / IN / BETWEEN.
- Three-valued NULL semantics preserved: a NULL on either side
  drops the bucket.
- No AND / OR / parens inside HAVING for the MVP — future ADR.

## Consequences

- No engine-side wiring beyond adding one `parse_having` call and
  passing `having.as_ref()` into `evaluate_grouped`. All four
  engines inherit the new clause.
- ORDER BY / LIMIT / OFFSET continue to parse but are discarded
  for grouped queries; a future ADR can let ORDER BY reference
  an aggregate or a HAVING-surviving bucket.
- Validated by `tests/base_having.rs` (13 cases: HAVING on
  COUNT / SUM / MAX / MIN / DISTINCT, HAVING on a group-key
  column with equality and `<>`, non-group-key column rejection,
  HAVING-without-GROUP-BY rejection, empty-after-HAVING,
  composition with WHERE, Domain + MVCC parity). The 15 ADR 0217
  GROUP BY tests still pass.
