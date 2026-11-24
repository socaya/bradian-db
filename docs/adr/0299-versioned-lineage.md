# ADR 0299 — Versioned semantic catalog with lineage staleness

## Status

Accepted. First slice of P1-3 (catalog + semantic layer).

## Context

`bradian_catalog::lineage::Lineage` already stores edges by
source UID and supports transitive-dependent computation,
but has no notion of *when* an edge was computed. An edge
from `A → B` recorded before `B`'s definition changed is
indistinguishable from one recorded after. The P1-3
acceptance requires "graph export; stale-lineage
detection."

## Decision

Add `VersionedLineage` as an additive wrapper beside
`Lineage`. The existing `Lineage` surface stays unchanged
for current callers.

```rust
pub struct VersionedLineage {
    graph:     Lineage,
    revisions: AHashMap<Uid, u64>,          // current rev per node
    recorded:  Vec<(LineageEdge, u64, u64)>, // (edge, from_rev, to_rev)
}

impl VersionedLineage {
    pub fn bump_revision(&mut self, uid: &Uid) -> u64 { ... }
    pub fn record_edge(&mut self, edge: LineageEdge) { ... }
    pub fn stale_edges(&self) -> Vec<&LineageEdge> { ... }
    pub fn export_dot(&self)  -> String { ... }
    pub fn export_json(&self) -> String { ... }
}
```

Semantics:

- `bump_revision(uid)` monotonically advances the node's
  revision counter. Every recorded edge whose endpoints
  were tagged at an older revision becomes **stale**.
- `record_edge` captures the current `(from_rev, to_rev)`
  at edge creation time.
- `stale_edges` returns edges for which either endpoint's
  current revision is strictly higher than the one
  recorded.
- `export_dot` emits GraphViz DOT: stale edges styled
  `style=dashed,color=red`. `export_json` emits a JSON
  object with `nodes`, `edges`, and `stale_edges` lists.

Scope limits:

- Monotonic in-memory counters only. No durable revision
  journal yet; revisions are lost across restarts and
  must be rebuilt by replaying catalog DDL.
- No automatic hook from catalog DDL into
  `bump_revision`; the caller / semantic compiler is
  responsible for calling it on observable changes.
- No graph-rewrite optimisations, no partitioned
  subgraphs. Flat edge list.

## Consequences

- Downstream consumers (incremental views, impact
  analysis) can filter out stale edges before trusting a
  transitive-closure result.
- 4 new unit tests cover: bumping a node marks outbound +
  inbound edges stale; fresh-after-record edges are not
  stale; DOT export renders dashed red for stale edges;
  JSON export round-trips to a readable structure.
- `Lineage` is unchanged; existing catalog consumers
  continue to work with zero adaptation.
