# ADR 0350 — Engine-sourced column types for scalar functions

## Status

Accepted. Closes the "scalar function projections" and
"`*Expr` aggregate variants" follow-ups explicitly
deferred by ADRs 0347 and 0348. With this slice every
G12-SQL projection shape except `alias.col` in grouped
joins reports engine-sourced column types.

## Context

ADR 0347 wired bare-column SELECTs; ADR 0348 added
scalar + grouped aggregates; ADR 0349 added set
operators. Scalar function projections (`SelectItem::Func`)
still dropped to the empty marker:

> **Scalar function projections** — JSON path, CAST,
> CONCAT, etc. still fall through to inference.
>
> **`*Expr` aggregate variants** (`MinExpr`, `MaxExpr`,
> `SumExpr`, `AvgExpr`, `CountColExpr`,
> `CountDistinctExpr`) — these evaluate arbitrary
> `FuncExpr` per row. Typing the function output
> statically requires the same scalar-function typing
> work listed in ADR 0347.

Concretely that meant `SELECT CAST(x AS TEXT) FROM t
WHERE 0=1` reported `WireType::Unknown` even though
the target type is literally in the syntax — a
legitimate correctness gap, not just cosmetic.

## Decision

Two new helpers in `crates/bradian-relational/src/sql.rs`:

```rust
pub(crate) fn func_result_type(fe: &FuncExpr, table: &Table) -> Option<DataType>;
pub(crate) fn func_arg_type(arg: &FuncArg, table: &Table) -> Option<DataType>;
```

`func_result_type` covers every `FuncExpr` variant:

| `FuncExpr` arm                   | Output `DataType`                              |
|----------------------------------|------------------------------------------------|
| `Upper` / `Lower` / `Trim`       | `Text`                                         |
| `Concat` / `Substr` / `Replace`  | `Text`                                         |
| `Length`                         | `Int64`                                        |
| `Abs(arg)`                       | `func_arg_type(arg)` (numeric pass-through)    |
| `Coalesce(args)`                 | first typable arg's type                       |
| `NullIf(a, _)`                   | type of `a`                                    |
| `Case { arms, else_result }`     | first arm's result type, else `else_result`    |
| `Cast { target }`                | explicit `target`                              |
| `BinOp { lhs, rhs }`             | `Decimal {..}` if either side is `Decimal`;    |
|                                  | else `Int64`                                   |
| `Now` / `TimestampAdd`           | `TimestampTz`                                  |
| `TimestampDiff`                  | `Int64` (whole seconds)                        |

`func_arg_type` resolves `ColumnRef` through the
catalog, matches on the `Value` variant for `Literal`,
and recurses into `Nested(FuncExpr)`.

Two integration points:

1. The main SELECT path in `sql::execute_impl_single`
   now accepts `SelectItem::Func(fe)` in its type
   loop (ADR 0347 only handled bare columns). The
   function type is appended via `func_result_type`.
2. `aggregate_result_type` (ADR 0348) delegates the
   `*Expr` aggregate variants to `func_result_type`:
   - `MinExpr(fe)` / `MaxExpr(fe)` → inner function's
     output type verbatim.
   - `SumExpr(fe)` / `AvgExpr(fe)` → same widening
     rule as the column-backed path (`Int32/Int64` →
     `Int64`, `Decimal {..}` → preserved).
   - `CountColExpr(_)` / `CountDistinctExpr(_)` →
     `Int64`.

Returning `None` anywhere in the recursion drops the
whole projection to the empty marker so the wire
layer falls back to ADR 0340 row inference for that
row — consistent with the behaviour the previous
three ADRs installed.

## Scope

**In scope:**
- `func_result_type` + `func_arg_type` covering every
  current `FuncExpr` and `FuncArg` variant.
- Main `SqlEngine::execute` SELECT path routes
  `SelectItem::Func` through `func_result_type`.
- `aggregate_result_type` now types all six `*Expr`
  aggregate variants.
- 3 new tests (70 wire tests total):
  1. `cast_reports_target_data_type_from_engine` — an
     empty-result `CAST(value AS TEXT)` reports
     `String` from the explicit target, not `Unknown`.
  2. `length_upper_now_report_fixed_types_from_engine`
     — `LENGTH → Int64`, `UPPER → String`,
     `NOW() → TimestampTz`.
  3. `binop_on_integers_reports_int64_from_engine` —
     integer `+` yields `Int64`.

**Out of scope** (dedicated follow-ups):
- **Decimal literal precision/scale** — a bare
  `Decimal` literal arrives without declared
  `precision`/`scale`. `func_arg_type` returns `None`
  for that case rather than fabricating one; a
  principled precision/scale inference (looking at
  siblings in the expression tree) is a separate slice.
- **Qualified `alias.col` in grouped join SELECT** —
  the last `name.contains('.')` short-circuit in the
  type loop. Resolving against the composite schema
  in `execute_join_grouped` is wiring already done
  for the non-grouped INNER/OUTER JOIN path.
- **Session-engine scalar function support** — the
  three session engines (`history_sql`, `domain_sql`,
  `mvcc_sql`) iterate `plain_projections` and don't
  accept `SelectItem::Func` in their SELECT shape
  today. Adding function support there is independent
  of column typing.

## Consequences

- Every G12-SQL projection shape the engine supports
  now reports catalog-/syntax-sourced column types
  through the wire layer except for the qualified-
  join grouped case and bare-`Decimal`-literal
  arithmetic.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 70 wire tests pass.
- Pre-existing `base_expression_bundle::cast_bad_target_rejected`
  failure remains unrelated.
