# ADR 0297 — EXPLAIN surface backed by the SqlLogicalNode tree

## Status

Accepted. Second slice of P1-2.

## Context

ADR 0296 shipped a real `SqlLogicalNode` plan tree for the
relational SQL subset. P1-2 acceptance explicitly rejects
EXPLAIN that renders from a text template — the tree is
the source of truth.

`bradian-planner::explain::ExplainPlan` already serves the
DHIS2 analytical plan; it is not shaped for the relational
operator subset.

## Decision

Add a sibling in `bradian-planner::sql_plan`:

```rust
pub struct SqlExplainNode {
    pub operator: String,
    pub details:  Vec<(&'static str, String)>,
    pub children: Vec<SqlExplainNode>,
}
pub fn explain(node: &SqlLogicalNode) -> SqlExplainNode;
impl std::fmt::Display for SqlExplainNode { ... }
```

`explain` is a structural, lossless tree derivation of the
plan — no text templating, no interpolation of the SQL
source string. The `Display` impl is a deterministic
pretty-printer used in snapshot-style assertions.

Scope limits:

- Plan-tree only. No row-count estimates, no cost
  annotations, no physical / parallel strategy choice —
  those are 0298 and later.
- Does not touch the DHIS2 `ExplainPlan` (analytical). The
  two EXPLAIN surfaces cover different crates of the query
  lifecycle; a future unification is possible but not
  forced now.
- No JSON serialization yet (if a consumer needs it, the
  struct is `Debug` and all fields are plain types — a
  serde impl is a one-line follow-up).

## Consequences

- EXPLAIN is backed by the plan tree, as required.
- 3 unit tests verify: 1-node, 2-node, deeply nested
  tree. Each test asserts both the `SqlExplainNode`
  structure AND the deterministic `Display` output so a
  future refactor does not drift the surface.
- No behaviour change for existing callers.
