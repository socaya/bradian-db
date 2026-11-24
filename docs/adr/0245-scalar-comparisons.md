# ADR 0245 — Bare-column scalar comparison operators

## Status

Accepted.

## Context

Master-SRS P0-3 lists SQL conformance gaps. ADR 0244 landed
`IS [NOT] NULL` and `IS [NOT] DISTINCT FROM` on bare non-JSON
columns. Other scalar comparisons (`=`, `<>`, `<`, `<=`, `>`,
`>=`) on non-JSON columns were still rejected — a user writing
`WHERE grade > 25` on a plain INT column hit the PK-equality
path and failed. This blocks most ordinary scalar WHERE
queries from working without a JSON column trick.

## Decision

- `parse_select_where` dispatches to the boolean-tree parser
  when the bare column is followed by `<` or `>` (which lex
  into `Token::Symbol('<')` / `Token::Symbol('>')` and cover
  `<`, `<=`, `<>`, `>`, `>=`). It also dispatches when the
  column is followed by `=` AND the column is not a
  primary-key column — this routes `WHERE non_pk = lit` to
  the matcher evaluator while preserving the PK-equality
  fast-path for `WHERE pk_col = lit [AND pk_col2 = lit …]`.
- `parse_json_predicate` accepts `Projection::Column` for the
  Compare matcher. Literal coercion uses `coerce(lit, column)`
  for bare columns so the RHS takes the column's declared
  type; JSON-path predicates keep `coerce_where_literal` with
  the existing `->` / `->>` as-text rules.
- Before the matcher dispatch, bare-column `IN` / `LIKE` /
  `ILIKE` / `BETWEEN` / `NOT …` are rejected explicitly so
  those matchers remain unsupported on non-JSON columns until
  a separate slice adds them.

## Consequences

- `WHERE col = lit`, `<> lit`, `< lit`, `<= lit`, `> lit`,
  `>= lit` now all work on bare non-JSON columns. Type
  mismatches surface as `SqlError::Type` through `coerce`.
  NULL propagates through comparisons (three-valued logic).
- PK-equality queries continue to use the single-row
  `KeyPredicate` fast path. PK columns compared with `<` / `>`
  go through the tree-scan evaluator — correct but no longer
  O(1).
- `!=` is not supported (the lexer doesn't recognize `!`);
  SQL-standard `<>` is the spelling for inequality.
- Bare-column `LIKE`, `IN`, `BETWEEN` remain unsupported —
  each is a follow-up P0-3 slice.
- 16 cases in `tests/scalar_compare.rs` cover: PK equality
  fast-path, non-PK eq, != rejected, `<>`, `<`, `<=`, `>`,
  `>=`, text compare, type mismatch rejected, NULL
  propagation, PK with `<`, AND chain, mix with IS NULL, LIKE
  still rejected, IN still rejected.
