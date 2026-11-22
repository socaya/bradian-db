# ADR 0208 — BETWEEN / NOT BETWEEN on JSON path WHERE

## Status

Accepted.

## Context

Scalar `>= low AND <= high` ranges are expressible via the ADR
0204 AND combinator but read awkwardly, and the parser has no
way to spot the empty-range `low > high` mistake. SQL's standard
`BETWEEN low AND high` shorthand is the natural fix, and the
same shape negates cleanly as `NOT BETWEEN`.

## Decision

Add `JsonMatcher::Between { low, high }` and `NotBetween { low,
high }` — both inclusive on each end to match SQL-99.

- Parser accepts `BETWEEN` and `NOT BETWEEN` after a JSON path
  LHS. The `AND` between `low` and `high` is a grammar keyword
  consumed before the DNF combinator loop reads the next
  AND / OR, so range AND does not merge the predicate with the
  following one.
- `parse_between_tail` reuses `coerce_where_literal` to honor the
  `->` / `->>` typing rule applied in the earlier matchers.
- Empty range (`low > high`) is rejected at parse with
  `SqlError::Syntax` — a legal empty query would be surprising
  and almost always a typo.
- NULL `low` or `high` → predicate evaluates to false (three-valued
  logic). Extracted NULL continues to short-circuit via the
  earlier NULL guard; only `IS [NOT] NULL` sees it directly
  (ADR 0207).

## Consequences

- No engine-side changes — `apply_where_filter` carries it for all
  four engines. Composes with AND / OR DNF (ADRs 0204/0205), IN /
  NOT IN (ADR 0206), and IS [NOT] NULL (ADR 0207).
- Validated by `tests/json_path_where_between.rs` (14 cases:
  numeric inclusive, boundary inclusion, text range, NOT BETWEEN,
  AND / OR composition, container form, empty-range rejection,
  missing-AND rejection, UPDATE / DELETE, composition with IN,
  and Domain + MVCC parity).
- No numeric-vs-text type enforcement at parse — we rely on the
  runtime comparison semantics in `compare_values_for_check` to
  match the same rules used by `=` / `<` / `>` elsewhere.
