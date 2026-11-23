# ADR 0234 — Nested function arguments

## Status

Accepted.

## Context

ADRs 0232 / 0233 landed single-level function calls like
`UPPER(name)` and `TRIM(name)`. Real queries need composition:
`UPPER(TRIM(name))`, `COALESCE(UPPER(label), 'MISS')`.

## Decision

Extend `FuncArg` with a `Nested(Box<FuncExpr>)` variant. The
parser `parse_func_arg` tries `consume_func` first so nested
function calls bind before column refs or literals.
`evaluate_func_arg` delegates `Nested` back to `evaluate_func`.

## Consequences

- Recursion depth is bounded by the parser's token budget; no
  explicit limit is applied in this ADR.
- Enables ADR 0237 (`SUM(ABS(col))`): aggregate variants parse
  through `consume_func` before falling back to a bare column
  name.
