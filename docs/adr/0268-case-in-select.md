# ADR 0268 — CASE in SELECT projection (verification)

## Status

Accepted.

## Context

`CASE WHEN ... THEN ... ELSE ... END` on scalar columns was
implemented in the function-expression grammar (ADRs 0233 /
0238) and already dispatches through `consume_func` →
`parse_select_item`. The P0-3 gap list called out "CASE in
SELECT" as open because it hadn't been verified end-to-end.
This ADR adds the tests and pins the existing behavior.

## Decision

- No new code. `FuncExpr::Case` reaches SELECT projection
  via the existing `consume_func` dispatch. CASE supports
  Compare, Between, and NotBetween WHEN clauses, with
  optional ELSE and multi-WHEN chains.
- CASE evaluation preserves three-valued NULL: a NULL
  predicate column makes the arm not match, so the next WHEN
  is tried.

## Consequences

- An existing cross-type width gap in `compare_values_for_check`
  (Int32 vs Int64 returns Equal) is visible in CASE too. A
  separate ADR closes that (P0-3 follow-up).
- 8 cases in `tests/case_in_select.rs` cover: Compare,
  Between, NotBetween, multi-WHEN first-match-wins, NULL
  falls to ELSE, no-ELSE returns NULL, zero-WHEN rejected,
  numeric result.
