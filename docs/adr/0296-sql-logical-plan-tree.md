# ADR 0296 — Logical plan tree for the existing SQL subset

## Status

Accepted. First slice of P1-2 (planner + binder parity).

## Context

`bradian-planner::plan` already has a `LogicalPlan`, but
it is DHIS2 DSL-oriented and bound to an analytical
`Partition` — it accepts DHIS2 `AggregateQuery` /
`ProgramIndicatorQuery` requests and produces aggregate
operators against columnar segments.

The relational SQL subset currently in
`bradian-relational::sql::SqlEngine` has no planner step;
parsing, binding and execution are intertwined. P1-2 asks
for an explicit SQL logical plan tree so that future
slices (EXPLAIN, cost-based reordering) have a real plan
to decorate rather than a rendered text template.

## Decision

Add a new module `bradian_planner::sql_plan` deliberately
separate from the DHIS2 `LogicalPlan`:

```rust
pub enum SqlLogicalNode {
    Scan      { source: String, column_names: Vec<String> },
    Filter    { input: Box<SqlLogicalNode>, predicate: FilterPredicate },
    Project   { input: Box<SqlLogicalNode>, keep: Vec<String> },
}

pub enum FilterPredicate {
    Eq { column: String, literal: Value },
    Gt { column: String, literal: Value },
    Lt { column: String, literal: Value },
}
```

Plus an `execute(node, rows, schema) -> Vec<Vec<Value>>`
interpreter that applies the tree bottom-up over an
in-memory row set carrying the input schema.

Honest scope:

- This slice ships the **plan tree and interpreter**.
  Automatic SQL-text → `SqlLogicalNode` compilation is a
  follow-up (ADR 0297 can close it, or a sub-slice).
- Projection, equality / inequality filter and scan are
  the only operator kinds. No joins, no aggregates, no
  sort, no limit — those are the next slices.
- Executes in-memory from a borrowed row set. Does NOT
  yet replace `SqlEngine`'s execution path; the parity
  comes from feeding the SQL engine's scanned rows into
  both the direct SQL execution (as the oracle) and an
  equivalently-constructed plan tree.
- No cross-crate behaviour change for existing callers.

## Consequences

- `bradian-planner` gains a non-DHIS2 plan tree that
  future slices (0297 EXPLAIN, 0298 join reorder) can
  refine without touching the DHIS2 planner.
- 1 integration test
  (`crates/bradian-relational/tests/sql_plan_tree_parity.rs`)
  runs three SQL statements through `SqlEngine::execute`
  and compares each result to a hand-built `SqlLogicalNode`
  tree executed over the same table rows. Parity holds.
- 1 planner-side unit test exercises the operator-tree
  bottom-up interpretation directly on synthetic rows.
- No format change, no API break.
