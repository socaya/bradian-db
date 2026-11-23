# ADR 0225 — UNION / UNION ALL

## Status

Accepted.

## Context

Combining the output of two or more SELECTs into a single
result set is the standard SQL idiom for "either" queries.

## Decision

Pre-process the SQL in `execute_impl` to detect top-level
`UNION` / `UNION ALL` boundaries and execute each arm
independently, combining results.

- `split_top_level_unions(sql)` walks the input respecting
  single-quoted string literals and parenthesis nesting. It
  returns `None` when no top-level UNION is present; otherwise
  a `Vec<(preceded_by_ALL, arm_sql)>` with word-boundary
  matching (so `'UNION of stuff'` inside a literal is never
  split).
- `execute_impl` calls `execute_impl_single` per arm; the
  per-arm execution reuses every existing SELECT pipeline
  (WHERE, GROUP BY, HAVING, ORDER BY, LIMIT/OFFSET, DISTINCT,
  aggregates, JSON paths).
- Combination rules:
  - Column counts must match across arms; mismatch →
    `SqlError::Unsupported`.
  - `UNION ALL` concatenates. `UNION` dedups the accumulated
    result using `dedup_rows` (same cross-type equality as
    GROUP BY / DISTINCT from ADR 0219) and sorts via
    `sort_distinct_output` for deterministic emission.
  - Any `UNION` without `ALL` anywhere in the chain forces
    a final dedup over the full accumulated result.
- Mutex handling: `execute_impl` splits BEFORE acquiring any
  lock; each arm's `execute_impl_single` takes and releases
  the operation mutex independently.

## Consequences

- Scope cut: outer `ORDER BY` / `LIMIT` across the whole
  UNION chain is not parsed yet. Each arm can have its own
  ORDER BY / LIMIT.
- Base `SqlEngine` only (the pre-processor runs at the
  outermost entry point). Transactional engines keep the
  single-SELECT shape until their execute paths adopt the
  same splitter.
- 9 cases in `tests/base_union.rs` cover UNION-dedups, UNION
  ALL-keeps, three-arm chain, mixed ALL + dedup, column-count
  mismatch rejection, per-arm WHERE, empty arms on both
  sides, quoted-literal UNION safety.
