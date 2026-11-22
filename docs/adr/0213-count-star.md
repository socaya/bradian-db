# ADR 0213 — SELECT COUNT(*) on the base engines

## Status

Accepted.

## Context

All four SQL engines surface typed row sets today — there is no
scalar aggregation. Callers that only want a row count have to
materialize every row and count client-side, which defeats the
admission budgets and wastes bandwidth.

## Decision

Introduce `COUNT(*)` as a top-level SELECT shape that owns the
projection list and collapses the filtered row stream into a
single-row result.

- `consume_count_star(&mut Parser)` runs at the head of the
  SELECT branch, before the wildcard `*` dispatch. It consumes
  exactly `COUNT ( * )` and returns `true`; a `COUNT` followed by
  anything else (e.g. `COUNT(id)`, `COUNT(DISTINCT …)`) errors
  with `SqlError::Unsupported` so future ADRs can add those
  variants without grammar churn.
- When `count_star` is true, each engine short-circuits after the
  WHERE/filter stage with
  `SqlResultSet::Rows { columns: ["count"], rows: [[Value::Int64(n)]] }`.
- `ORDER BY` / `LIMIT` / `OFFSET` parse successfully but are
  discarded: a scalar aggregate has no per-row ordering.
- Mixing `COUNT(*)` with other projections (comma after the
  closing `)`) fails at the subsequent projection parse. No
  `GROUP BY`, no multi-aggregate yet.

## Consequences

- Carries through `SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`,
  and `HistorySqlEngine`. Admission budgets still charge the
  pre-filter row/byte count — the collapse happens after charging.
- Validated by `tests/base_count_star.rs` (12 cases: empty table,
  full scan, PK WHERE match / miss, JSON path WHERE, IN list,
  LIKE, `COUNT(col)` and `COUNT(*), id` rejection, ORDER BY
  ignored, Domain + MVCC parity).
- Future ADRs can layer `COUNT(col)`, `COUNT(DISTINCT col)`,
  `SUM / AVG / MIN / MAX`, and eventually `GROUP BY` on top of
  this shape.
