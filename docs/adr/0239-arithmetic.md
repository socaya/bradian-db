# ADR 0239 — Arithmetic infix in expressions

## Status

Accepted.

## Context

The scalar function / CASE path (ADRs 0232–0238) accepts
columns, literals, and nested function calls as arguments.
Common SQL queries also require arithmetic composition:
`SELECT price * quantity`, `CASE WHEN score + 5 > 10 THEN …`,
`SELECT SUM(qty * price) FROM …`. Closes a visible P0-3 SQL
conformance gap.

## Decision

- `FuncExpr::BinOp { op: char, lhs, rhs }` where `op` is one
  of `+ - * / %`.
- Lexer admits `+ - / %` as standalone symbols in every mode.
  Numeric literals never include a leading sign at the lexer
  level — `-5` emits `Symbol('-'), Number("5")` which
  `Parser::literal` collapses into `Literal::Number("-5")`.
  `col - 5` emits `Word("col") Symbol('-') Number("5")` which
  the new arithmetic parser interprets as subtraction.
- `parse_func_arg` now walks a two-tier precedence:
  `parse_func_primary` → `parse_func_multiplicative`
  (`* / %`) → additive (`+ -`). Left-associative at each tier.
- `parse_select_item` routes bare-column projections into the
  arithmetic layer when an infix operator follows; JSON-path
  projections stay on their existing path and reject infix
  (`SqlError::Unsupported`) pending a future ADR.
- `consume_aggregate` parses each aggregate argument through
  `parse_func_arg`. A bare `FuncArg::ColumnRef` lands on the
  existing `AggSpec::{Min, Max, Sum, Avg, CountCol,
  CountDistinct}` string variants; a `FuncArg::Nested`
  (nested function, arithmetic, etc.) lands on the `Expr`
  variants added in ADR 0237.
- Evaluator: `numeric_pair` widens Int32 / Int64 / Decimal to
  `i128`; Decimal operands propagate so Decimal × Int keeps a
  fixed-scale result. Checked arithmetic surfaces
  `SqlError::Type` on overflow, divide-by-zero, or mod-by-zero.
  NULL on either side propagates to NULL.

## Consequences

- Column-containing text or boolean operands reject at eval
  time (`SqlError::Type`).
- `SUM(ABS(col))`, `AVG(LENGTH(name)) * 2`, `CASE WHEN qty +
  5 > 7 THEN …` all parse now that `parse_func_arg` recurses
  through arithmetic.
- Starts the SQL conformance P0-3 cleanup. Future ADRs for
  unary `NOT`, `IS DISTINCT FROM`, parenthesized boolean
  expressions, and arithmetic on JSON paths remain.
- Lexer change unifies the non-expression and expression modes
  for arithmetic; the `starts_with('-')` branches in several
  places are no longer hit at runtime but remain for safety.
- 11 new tests in `tests/base_arithmetic.rs` cover + - * / %,
  precedence, NULL propagation, divide-by-zero, arithmetic in
  CASE predicates, inside aggregates, negative literals, and
  text-operand rejection. The 442 lib + 158 integration tests
  continue to pass.
