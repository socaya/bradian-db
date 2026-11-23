# ADR 0233 — CASE WHEN + CONCAT / SUBSTR / TRIM / REPLACE

## Status

Accepted.

## Context

ADR 0232 added UPPER / LOWER / LENGTH / ABS / COALESCE / NULLIF.
Common query shapes still need pattern-based value selection
(`CASE`) and the standard string transforms missing from the
initial function set.

## Decision

Extend `FuncExpr` with:

- `Concat(Vec<FuncArg>)` — variadic text concat; NULL propagates;
  non-text scalars (`Int32`, `Int64`, `Decimal`, `Boolean`)
  auto-stringify so templates like `CONCAT(id, '-', name)` work.
- `Substr { s, start, length }` — SQL-standard 1-based start;
  non-positive start clamps to 1; omitted / non-positive length
  yields the empty string; NULL in any argument propagates.
- `Trim(arg)` — ASCII whitespace strip on both sides.
- `Replace { s, from, to }` — every occurrence substituted;
  empty `from` rejected (SQL avoids infinite expansion).
- `Case { arms, else_result }` — `CASE WHEN expr op lit THEN
  value [WHEN …] [ELSE value] END` with the existing `CheckOp`
  comparator set. First-match evaluation; no ELSE returns NULL.

Grammar: each function name is recognized in `consume_func` via
a new `p.consume_word` arm. CASE arms take an expression that
compiles to `(FuncArg column, CheckOp op, Value literal,
FuncArg result)` — predicates are restricted to the same
column/literal shape as scalar WHERE for the MVP.

## Consequences

- No nested function calls inside function arguments yet —
  arguments are column references or inline literals. CASE
  predicates share the same restriction; nested CASE or
  function-in-arg is a future extension.
- CONCAT auto-stringification is an explicit scope decision;
  SQL-92 is stricter (requires explicit CAST). We follow
  PostgreSQL / SQLite ergonomics for the MVP.
- 12 cases in `tests/base_case_and_strings.rs` cover CASE
  score-classify / no-ELSE-returns-NULL / text-column CASE,
  CONCAT columns + literals + NULL propagation, SUBSTR 2-arg /
  3-arg / out-of-range clamp, TRIM, REPLACE substitute +
  empty-from rejection, CASE + COALESCE mixed-projection.
  The 11 ADR 0232 tests still pass.
