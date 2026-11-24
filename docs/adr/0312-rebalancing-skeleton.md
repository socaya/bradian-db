# ADR 0312 — Shard rebalancing: skeleton

## Status

Accepted as a skeleton. Fifth slice of P2.

## Context

Clusters need to shift shard ownership when nodes join /
leave / skew. Full move protocol with handoff + catch-up
is Gate-sized.

## Decision

New module `bradian_distributed::rebalancing` with a
`MoveShard` plan item, a `RebalancePlan` builder from a
current → desired placement, and an `apply_to` that
updates a `PlacementMap` while advancing fence tokens.

```rust
pub struct MoveShard { pub shard: ShardId, pub from: ReplicaId, pub to: ReplicaId }
pub struct RebalancePlan { pub moves: Vec<MoveShard> }
impl RebalancePlan {
    pub fn diff(current: &PlacementMap, desired: &HashMap<ShardId, ReplicaId>) -> Self;
    pub fn apply_to(&self, map: &PlacementMap);
}
```

Honest scope: pure planning + in-memory apply. No data
movement, no catch-up, no handoff.

## Consequences

- 2 unit tests: diff produces only the shards whose
  primaries differ; apply advances the fence on each
  moved shard.
