# ADR 0219 — SELECT DISTINCT

## Status

Accepted.

## Context

The engines produce every raw row matching WHERE, which is
correct but leaves ad-hoc "unique values of X" queries to be
hand-written as `GROUP BY X`. SQL's `SELECT DISTINCT` is the
idiomatic spelling and does not require GROUP BY's
projection-restriction rules.

## Decision

Introduce `SELECT DISTINCT <projection-list> FROM …` with
per-row-tuple deduplication.

- `distinct = p.consume_word("distinct")` runs immediately
  after the `SELECT` keyword, before the projection list or
  aggregate head.
- `DISTINCT` is orthogonal to the aggregate / grouping pipeline:
  combining it with any aggregate or a non-empty GROUP BY yields
  `SqlError::Unsupported`. (If callers want distinct groups of
  aggregates, they already have GROUP BY.)
- After projection, `dedup_rows` scans the output tuples and
  drops any tuple that compares equal to an earlier one via
  `compare_values_for_check` — the same cross-type equality
  rule used by `COUNT(DISTINCT col)` (ADR 0216) and GROUP BY
  (ADR 0217).
- `sort_distinct_output` sorts the deduped list in ascending
  tuple order with NULL last at each position, matching the
  GROUP BY emission convention so callers get deterministic
  results without writing ORDER BY.
- ORDER BY / LIMIT / OFFSET still parse and apply before the
  dedup. Because they operate on raw table rows, their results
  are consistent as long as callers project the columns they
  ordered on.

### Companion fix: `compare_values_for_check` NULL handling

`compare_values_for_check` previously returned `Equal` for any
mismatched type pair, including `(Null, Int64)`. This was never
hit by the scalar WHERE / IN / BETWEEN paths because they
short-circuit NULL above the call, but `dedup_rows` and the
GROUP BY bucket comparator do not. The function now treats NULL
as:
- `(Null, Null)` → `Equal`
- `(Null, _)` → `Less`
- `(_, Null)` → `Greater`

so two NULLs collapse to one distinct value and a NULL is
distinct from any concrete value. Scalar matchers still filter
NULL out before comparing, so their behavior is unchanged.

## Consequences

- Carries through `SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`,
  and `HistorySqlEngine` via a one-line `distinct` flag and a
  `dedup_rows` + `sort_distinct_output` pair at the end of the
  projection pipeline.
- No multi-column dedup beyond the projected tuple. A future
  ADR can add `DISTINCT ON (col1, col2) …` (PostgreSQL extension).
- Validated by `tests/base_distinct.rs` (12 cases: single
  column, two columns, numeric, no-op when already unique, PK
  WHERE, empty result, aggregate / GROUP BY rejection, NULL
  handling, wildcard, Domain + MVCC parity). The 231 earlier
  aggregate / WHERE / GROUP BY / HAVING / COUNT tests continue
  to pass with the refined NULL semantics.
