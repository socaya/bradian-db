# ADR 0217 — GROUP BY with aggregate projections

## Status

Accepted.

## Context

ADRs 0213–0216 added `COUNT(*)`, `COUNT(col)`, `COUNT(DISTINCT col)`,
`MIN`, `MAX`, `SUM`, `AVG` as scalar aggregates that collapse the
filtered scan to a single row. Real queries also need per-bucket
reduction: `SELECT region, SUM(qty) FROM sales GROUP BY region`.

## Decision

Introduce a mixed-projection SELECT list and a `GROUP BY` clause.

- `SelectItem::{ Column(Projection), Agg(AggSpec) }` replaces the
  old `Option<AggSpec> + Vec<Projection>` split at the engine
  level. `parse_select_item` dispatches to either
  `consume_aggregate` or `parse_projection`.
- `parse_group_by` parses `GROUP BY col[, col]*` after WHERE. Each
  key must resolve to a bare table column (no JSON paths, no
  expressions, no aliases). Bounded to 16 keys; duplicates
  rejected.
- Planning rules (enforced in every engine's SELECT branch):
  - If `GROUP BY` is present, every `SelectItem::Column` must be
    a bare column listed in the keys; JSON paths rejected.
  - If any projection is an aggregate and `GROUP BY` is absent:
    all projections must be aggregates, and the single-aggregate
    scalar short-circuit (ADR 0213) still owns the output. Any
    other mix is `SqlError::Unsupported`.
- `evaluate_grouped` buckets rows by the composite group-key
  tuple using `compare_values_for_check` (same cross-type rule
  as `COUNT(DISTINCT col)`). Buckets are emitted in ascending
  group-key order; NULL sorts last. Bounded to 10 000 output
  groups to match the scan admission cap.
- Each engine (`SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`,
  `HistorySqlEngine`) resolves `SelectItem` → per-row `Projection`
  for the plain-row path and dispatches to `evaluate_grouped`
  when `group_keys` is non-empty.

## Consequences

- `ORDER BY / LIMIT / OFFSET` continue to parse but are
  discarded for grouped queries; grouped output is already
  sorted by key. A future ADR can let `ORDER BY` reference an
  aggregate column.
- `HAVING` is not implemented yet — a future ADR will add a
  post-bucket predicate.
- Validated by `tests/base_group_by.rs` (15 cases: single-key /
  two-key, multi-aggregate per bucket, projection-not-in-key
  rejection, missing-GROUP-BY mixed rejection, unknown-column
  and duplicate-key rejection, PK WHERE, empty-filter, AVG
  per-bucket, `COUNT(DISTINCT col)` per bucket, Domain + MVCC
  parity). All 121 earlier WHERE / aggregate / ORDER BY / COUNT
  tests still pass.
