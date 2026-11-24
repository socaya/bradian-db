# ADR 0246 — Bare-column IN / BETWEEN / LIKE / ILIKE

## Status

Accepted.

## Context

Master-SRS P0-3 lists SQL conformance gaps. ADRs 0244 and
0245 landed `IS` and comparison operators on bare non-JSON
columns. `IN`, `NOT IN`, `BETWEEN`, `NOT BETWEEN`, `LIKE`,
`NOT LIKE`, `ILIKE`, `NOT ILIKE` on bare columns remained
unsupported, blocking most set-membership, range, and
pattern queries on plain scalar columns.

## Decision

- New helper `coerce_rhs_literal(lit, Option<&Column>,
  as_text)` routes literal coercion by column: `Some(col)`
  uses `coerce(lit, col)`; `None` uses the existing
  `coerce_where_literal(lit, as_text)`.
- `parse_literal_list` and `parse_between_tail` take the
  new `Option<&Column>` handle and thread it through.
- `parse_json_predicate` computes `column_ref` once
  (`Some(&table.columns[col_idx])` when `is_bare`, else
  `None`) and passes it to the matcher branches. Compare,
  IsDistinctFrom, IN/NOT IN, BETWEEN/NOT BETWEEN all use the
  routed helper. LIKE/ILIKE and their NOT variants call a
  `check_text_column` closure that rejects non-Text/Varchar
  columns with `SqlError::Type`.
- `parse_select_where` now also dispatches bare-column
  WHERE to the tree parser when the next word after the
  column is `in` / `not` / `like` / `ilike` / `between`.
- The pre-matcher bare-column reject (ADR 0245) is removed —
  every matcher now has an explicit bare-column path.

## Consequences

- `WHERE col IN (…)`, `NOT IN (…)`, `BETWEEN low AND high`,
  `NOT BETWEEN`, `LIKE pattern`, `NOT LIKE`, `ILIKE`,
  `NOT ILIKE` all work on bare non-JSON columns. Literal
  type mismatches and LIKE on non-text columns surface as
  `SqlError::Type`.
- Mixing JSON-path and bare-column predicates in a boolean
  tree (e.g. `body ->> 'status' = 'open' AND id = 1`) now
  works correctly. One existing canary test
  (`and_followed_by_pk_rejected` in `json_path_where_and.rs`)
  asserted the old buggy "mix rejected" behavior; it is
  updated to assert the new correct semantics.
- Scalar `=` / `<>` / `<` / `<=` / `>` / `>=` from ADR 0245
  were already covered.
- 16 cases in `tests/scalar_in_between_like.rs` cover IN /
  NOT IN (numeric + text + type mismatch), BETWEEN / NOT
  BETWEEN (numeric + text + low>high rejected), LIKE / NOT
  LIKE / ILIKE (incl. case-insensitivity, non-text column
  rejected), AND/OR composition with other matchers, PK IN
  through tree parser.
