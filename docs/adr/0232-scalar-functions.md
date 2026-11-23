# ADR 0232 — Scalar text / null / math functions in SELECT

## Status

Accepted.

## Context

Every projection today is either a column or a scalar
aggregate. Common scalar transforms like `UPPER(name)`,
`COALESCE(nickname, name)`, and `ABS(qty)` require an inline
function path.

## Decision

Introduce `SelectItem::Func(FuncExpr)` as a third projection
shape. The initial set:

- Text: `UPPER(col|lit)`, `LOWER(col|lit)`, `LENGTH(col|lit)`.
- Null: `COALESCE(arg, arg, …)`, `NULLIF(a, b)`.
- Math: `ABS(col|lit)`.

Each argument is a `FuncArg::{ColumnRef(String), Literal(Value)}`.
Column references accept the dotted `alias.col` form so the
same parser works in joined queries (future extension — the
MVP scalar path uses single-table resolution).

Evaluation (`evaluate_func`) runs against the full table row;
NULL propagates through UPPER / LOWER / LENGTH / ABS. COALESCE
returns the first non-NULL argument. NULLIF returns NULL when
both are non-NULL and equal; otherwise returns the first
argument.

Base-engine SELECT is rewired: when any projection is a Func,
the final loop skips `project_rows_with_control` and evaluates
each item directly against the full row.

## Consequences

- Grouped SELECTs reject function projections for now
  (`SqlError::Unsupported`) — nesting a function around an
  aggregate (`UPPER(MAX(name))`) deserves its own ADR.
- Column-level policy admits `SelectItem::Func(_)` without
  further check — the function may reference columns, but the
  policy evaluator operates on raw `SelectItem::Column(pj)`.
  Column-level denial of a function's inner column reference
  is a future extension.
- No type promotion / casting — numeric COALESCE requires
  matching concrete types already.
- 11 cases in `tests/base_scalar_functions.rs` cover UPPER /
  LOWER / LENGTH / ABS, COALESCE column-chain and trailing
  literal, NULLIF equal and unequal, NULL propagation,
  non-text argument rejection, mixed bare-column + function
  projection.
