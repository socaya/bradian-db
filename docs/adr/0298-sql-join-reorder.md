# ADR 0298 — Cost-based join reordering (two-table + chained)

## Status

Accepted. Third slice of P1-2 (planner + binder parity).

## Context

With the SqlLogicalNode tree and EXPLAIN surface in place
(0296 / 0297), the planner can host its first genuine
decision: given an inner equi-join, build the smaller side
first.

An interpreter that always builds the left side of a join
regardless of cardinality pays the cost of its own worst
input order. The P1-2 acceptance for this slice explicitly
calls for a cost model that demonstrates a gain.

## Decision

Extend `SqlLogicalNode` with a single new operator — inner
equi-join — and a reorder step before interpretation:

```rust
pub enum SqlLogicalNode {
    Scan      { ... },
    Filter    { ... },
    Project   { ... },
    Join      { left: Box<...>, right: Box<...>, on: JoinOn },
}
pub struct JoinOn { pub left_column: String, pub right_column: String }
```

Cost model:

- `estimated_rows(node, stats) -> usize` where `stats` is
  `HashMap<source, usize>` naming the row count of each
  Scan source. Filter applies a static selectivity of
  **0.5** (invertible aggregates do not change cardinality
  substantially; no histogram yet). Project is identity.
  Join returns `max(left, right)` as a conservative upper
  bound of the build-side work.

Reorder step:

- `reorder_joins(node, stats)` is a bottom-up rewrite that
  for every `Join` compares the estimated rows of the two
  sides and places the smaller side as `left` so the build
  phase is cheaper. The on-condition's columns are swapped
  to match. The rewrite preserves semantics of inner
  equi-join (commutative) exactly.

Interpreter:

- `execute` adds a Join case that computes an in-memory
  equi-join by building a hash map keyed on
  `on.left_column` of the left input, then probing with
  `on.right_column` of the right input. Output schema
  concatenates the two input schemas; output rows
  concatenate matched row pairs. No projection pushdown
  yet.

Honest scope:

- Only inner equi-joins. Outer, semi, anti, non-equi and
  cross joins are future slices.
- Only Scan sources carry statistics for this release.
  Statistics are supplied by the caller; the catalog has
  no row-count field yet.
- "Benchmark demonstrates gain" is proven in-test by
  counting hash-build operations under both orderings for
  a deliberately asymmetric input — a Criterion bench is a
  follow-up (ADR 0307).

## Consequences

- The planner now makes a real, measurable cost decision.
- 3 new unit tests cover: two-table reorder swaps sides
  when the right is smaller; a chained three-table join
  rewrites both nested joins; join interpreter output is
  the same regardless of reorder (commutativity).
- 1 property-style test asserts that the reordered plan
  does strictly fewer build-hash probes than the
  unreordered plan on the asymmetric workload.
- `SqlExplainNode` automatically covers Join because its
  pretty-printer walks children generically.
