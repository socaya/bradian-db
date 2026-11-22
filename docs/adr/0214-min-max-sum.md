# ADR 0214 — MIN, MAX, SUM scalar aggregates

## Status

Accepted.

## Context

ADR 0213 shipped `COUNT(*)` as a top-level SELECT shape that
collapses a filtered scan into a single-row result. Range and
numeric queries also need `MIN`, `MAX`, and `SUM` to avoid
materializing every row for client-side reduction.

## Decision

Promote the aggregate marker from a boolean (`count_star`) to an
`AggSpec` enum carrying the scalar aggregate and its target
column, then reuse the same single-row short-circuit across all
four engines.

- `consume_count_star` becomes `consume_aggregate` returning
  `Option<AggSpec>`. Supported heads: `COUNT(*)`, `MIN(col)`,
  `MAX(col)`, `SUM(col)`. Everything else keeps the pre-existing
  parse error for `COUNT(col)` / `COUNT(DISTINCT …)`.
- `evaluate_aggregate(spec, table, rows)` runs after
  `apply_where_filter`:
  - `COUNT(*)` → `Value::Int64(rows.len())`.
  - `MIN` / `MAX` iterate with `compare_values_for_check`,
    skipping `Value::Null`. All-null → `Value::Null`.
  - `SUM` accumulates into `i128` for overflow safety. Non-
    numeric columns are rejected at evaluation time with
    `SqlError::Type`; overflow on the final pack to `Int32` /
    `Int64` also rejects. `Decimal` columns preserve the
    column's declared scale on the way out.
- Each of `SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`,
  `HistorySqlEngine` dispatches on `aggregate.is_some()` after
  the WHERE filter and before projection / ordering. ORDER BY /
  LIMIT / OFFSET continue to parse but are discarded for
  aggregate shapes.

## Consequences

- SUM widens `Int32` sums to `Int64` so adding a million Int32
  values cannot silently wrap into a negative Int32.
- No `COUNT(col)`, no `AVG`, no `DISTINCT`, no multi-aggregate
  projection, no GROUP BY — each a future ADR.
- Validated by `tests/base_min_max_sum.rs` (14 cases: numeric
  MIN/MAX/SUM, text MIN/MAX, NULL skipping, all-NULL result,
  empty table result, non-numeric SUM rejection, PK WHERE hit,
  PK WHERE miss, unknown-column rejection, Domain + MVCC
  parity). The 12 ADR 0213 COUNT(*) tests still pass.
