# ADR 0216 — COUNT(col) and COUNT(DISTINCT col)

## Status

Accepted.

## Context

ADR 0213 shipped `COUNT(*)` as the only `COUNT` shape and
rejected `COUNT(col)` / `COUNT(DISTINCT …)` to preserve grammar
space. With `MIN`, `MAX`, `SUM`, `AVG` landed (ADRs 0214/0215),
the natural completeness item is the two remaining COUNT forms.

## Decision

Extend `AggSpec` with `CountCol(column)` and
`CountDistinct(column)` and dispatch on the token after the
opening `(` in `consume_aggregate`.

- Grammar inside the `COUNT` branch:
  - `COUNT(*)` → `AggSpec::CountStar` (unchanged).
  - `COUNT(DISTINCT col)` → `AggSpec::CountDistinct`.
  - `COUNT(col)` → `AggSpec::CountCol`.
- `CountCol` iterates filtered rows and increments once per
  non-NULL value at the target column.
- `CountDistinct` builds a bounded de-duplication buffer using
  `compare_values_for_check` so cross-type equivalents collapse
  into the same bucket (e.g. `Value::Json("42")` and legacy
  `Value::String("42")` representations of the same stored JSON,
  matching the scalar comparison semantics used everywhere
  else).
- Both shapes return `Value::Int64(n)` with result column
  `"count"`; empty / all-NULL inputs return `0` (not `NULL`,
  matching SQL semantics for `COUNT`).

## Consequences

- All four engines pick this up for free via
  `evaluate_aggregate`. ORDER BY / LIMIT / OFFSET continue to
  parse but are discarded (scalar aggregate).
- `CountDistinct` is `O(n * d)` where `d` is the distinct-value
  count; good enough for scan-bounded admission caps, and the
  cross-type equality cannot easily use a hash set without
  introducing an explicit type-normalized key.
- The ADR 0213 test asserting `SELECT COUNT(col)` was rejected is
  stale and refreshed to assert the non-null count equals
  `COUNT(*)` when the target column is `NOT NULL`.
- Validated by `tests/base_count_col.rs` (13 cases: nullable
  text / numeric non-null counts, NOT NULL column = COUNT(*),
  distinct text / numeric, empty table, all-NULL, PK WHERE,
  DISTINCT with WHERE, unknown-column rejection (both forms),
  Domain + MVCC parity). The 13 COUNT(*) and 14 MIN/MAX/SUM and
  13 AVG tests still pass.
