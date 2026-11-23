# ADR 0237 — Aggregates over scalar expressions

## Status

Accepted.

## Context

`SUM(col)`, `AVG(col)`, `MIN(col)`, … accept a bare column.
Common patterns like `SUM(ABS(col))` or `COUNT(DISTINCT
CAST(s AS INT))` need to feed a scalar expression into the
aggregator.

## Decision

Add parallel `Expr` variants to `AggSpec`:
`MinExpr(FuncExpr)`, `MaxExpr(FuncExpr)`, `SumExpr(FuncExpr)`,
`AvgExpr(FuncExpr)`, `CountColExpr(FuncExpr)`,
`CountDistinctExpr(FuncExpr)`.

`consume_aggregate` tries `consume_func` first inside each
aggregate's parentheses; on success stores the `Expr` variant,
otherwise falls back to the pre-existing `String` column name.

`evaluate_aggregate` grows matching branches that evaluate the
expression per row and feed results through the same
accumulators used by the column-based variants:

- `MinExpr`/`MaxExpr`: NULL-skipping reduce with
  `compare_values_for_check`.
- `SumExpr`: `i128` accumulator; Decimal operands preserve
  the fixed-scale representation; Int operands widen to Int64.
- `AvgExpr`: SUM / COUNT with half-even rounding; Decimal
  operands preserve scale.
- `CountColExpr`/`CountDistinctExpr`: NULL-skip, dedup via
  cross-type `compare_values_for_check`.

HAVING inherits the new variants automatically because it
stores `AggSpec`.

## Consequences

- No explicit per-value type check against a column schema for
  Expr variants — the expression's own evaluator surfaces type
  errors. Non-numeric SUM/AVG arguments still reject.
- GROUP BY projections can now mix aggregates over expressions
  with group-key columns through the existing
  `evaluate_grouped` dispatch.
- Reference tests live in `tests/base_expression_bundle.rs`
  alongside ADRs 0234–0238.
