# ADR 0348 — Engine-sourced column types for aggregates

## Status

Accepted. Extends ADR 0347 to the scalar aggregate and
GROUP BY output sites, so the wire layer no longer has
to fall back to row inference for the COUNT/MIN/MAX/
SUM/AVG column shape across the four SQL engines.

## Context

ADR 0347 landed engine-sourced `column_types` on every
bare-column SELECT path. Aggregate outputs explicitly
fell back to inference with a dedicated follow-up note:

> **Aggregate column typing** — the GROUP BY output site
> populates types for group-key columns only; aggregate
> columns (COUNT, SUM, AVG, MIN, MAX) and scalar
> functions fall back to inference. A principled type
> derivation (COUNT → Int64, MIN/MAX → source type, SUM
> over int → Decimal, AVG → Decimal) is the next slice.

That follow-up is this slice. The same two pathological
cases ADR 0347 fixed for bare columns —

1. An empty result set produces `Unknown` columns
   because there's nothing to infer from.
2. An all-null column produces `Unknown` for the same
   reason.

— both still applied to aggregates before this slice
(e.g. `SELECT SUM(value)` over an all-null column
reported `Unknown`).

## Decision

Add `aggregate_result_type(spec, table) -> Option<DataType>`
in `crates/bradian-relational/src/sql.rs`, mirroring the
Value variant that `evaluate_aggregate` produces:

| `AggSpec`                        | Declared `DataType`                              |
|----------------------------------|--------------------------------------------------|
| `CountStar`                      | `Int64`                                          |
| `CountCol(c)` / `CountDistinct(c)`| `Int64`                                         |
| `Min(c)` / `Max(c)`              | `table.columns[locate(c)].data_type`             |
| `Sum(c)` on `Int32/Int64`        | `Int64`                                          |
| `Sum(c)` on `Decimal {..}`       | same `Decimal` (scale preserved)                 |
| `Avg(c)` on `Int32/Int64`        | `Int64` (truncated)                              |
| `Avg(c)` on `Decimal {..}`       | same `Decimal` (scale preserved)                 |
| `*Expr(_)` variants              | `None` — arbitrary `FuncExpr`, falls back        |

Wire it into the five call sites that currently return
`SqlResultSet::Rows` for aggregate output:

1. `sql::execute_impl_single` scalar aggregate branch
2. `sql::execute_join_grouped` scalar aggregate branch
3. `sql::evaluate_grouped` (now covers `SelectItem::Agg`
   in addition to the ADR 0347 bare-column path)
4. `history_sql::HistorySqlSession` scalar aggregate
5. `domain_sql::DomainSqlSession` scalar aggregate +
   `mvcc_sql::MvccSqlSession` scalar aggregate

Each site is a one-line change: `column_types =
aggregate_result_type(&spec, &table).map(|t|
vec![t]).unwrap_or_default()`.

The GROUP BY site's helper now descends into
`SelectItem::Agg(spec) => aggregate_result_type(spec,
table)`; one un-typable item (a `*Expr` variant or a
scalar function) still drops the entire row to
`Vec::new()` so downstream inference stays consistent.

## Scope

**In scope:**
- `aggregate_result_type` helper covering every
  statically-typable `AggSpec` variant.
- All five scalar-aggregate emission sites across the
  four SQL engines, plus the GROUP BY output helper.
- 3 new tests (65 wire tests total):
  1. `count_star_reports_int64_from_engine_without_inference_on_empty_table`
     — `SUM(value)` over an all-null BIGINT column
     reports `Int64` (not `Unknown`).
  2. `min_max_report_source_column_type_from_engine`
     — two single-aggregate SELECTs for `MIN(value)`
     and `MAX(value)` each report `[Int64]`.
  3. `group_by_bucket_count_reports_int64_from_engine`
     — `SELECT id, COUNT(*) GROUP BY id` reports
     `[Int64, Int64]` with both columns engine-typed.

**Out of scope** (dedicated follow-ups):
- **`*Expr` aggregate variants** (`MinExpr`, `MaxExpr`,
  `SumExpr`, `AvgExpr`, `CountColExpr`,
  `CountDistinctExpr`) — these evaluate arbitrary
  `FuncExpr` per row. Typing the function output
  statically requires the same scalar-function typing
  work listed in ADR 0347.
- **Scalar function projections** — JSON path, CAST,
  CONCAT, etc. still fall through to inference.
- **Set-operator result typing** — UNION/INTERSECT/
  EXCEPT, per ADR 0347 follow-up.
- **Qualified `alias.col` in grouped join SELECT** —
  currently drops to empty under the
  `SelectItem::Column(Projection::Column(name))` arm
  because `name` contains `.`.

## Consequences

- An empty-result or all-null aggregate SELECT reports
  the catalog-declared column type instead of
  `WireType::Unknown`.
- Clients building typed cursors for aggregates no
  longer need per-cell wire-type sniffing on the fast
  path.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 65 wire tests pass.
- Pre-existing `base_expression_bundle::cast_bad_target_rejected`
  failure remains unrelated (reproduces on main
  without this slice).
