# ADR 0275 — INTERSECT / EXCEPT set operators

## Status

Accepted.

## Context

ADR 0225 added `UNION` / `UNION ALL` at the top level. The
P0-3 gap plan listed `INTERSECT` and `EXCEPT` as still open.
Both are set-theoretic complements of UNION with the same
column-count + dispatch shape, so they fit naturally on top
of the existing splitter.

## Decision

- New `SetOp { Union, UnionAll, Intersect, Except }` enum.
- New `split_top_level_set_ops(sql)` — a superset of
  `split_top_level_unions` that also detects INTERSECT and
  EXCEPT. UNION-only queries continue to use the old
  splitter so ADR 0225 behavior is unchanged.
- New `execute_set_ops(parts, …)` applies operators
  left-associatively against the running accumulator:
  - UNION ALL — extend.
  - UNION — extend + dedup + sort.
  - INTERSECT — keep rows present in both (dedup + sort).
  - EXCEPT — keep rows in left not in right (dedup + sort).
- `execute_impl` dispatches to `execute_set_ops` only when
  the split result contains a non-UNION separator; pure UNION
  chains stay on the ADR 0225 fast path.

## Consequences

- INTERSECT / EXCEPT are O(n*m) on row count — acceptable
  for MVP since `Value` doesn't implement `Hash`. A future
  slice can add `Hash` or sort-merge semantics.
- Mixed chains `A INTERSECT B UNION C` are left-associative
  per SQL-standard precedence note (callers wanting a
  different grouping must parenthesise, which the current
  splitter already treats as a sub-SELECT).
- 5 cases in `tests/set_ops.rs` cover: INTERSECT keeps
  common; EXCEPT subtracts; EXCEPT distinct; mixed
  INTERSECT + UNION; INTERSECT with no overlap.
