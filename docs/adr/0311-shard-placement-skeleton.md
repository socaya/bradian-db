# ADR 0311 — Shard placement + fencing: skeleton

## Status

Accepted as a skeleton. Fourth slice of P2.

## Context

Multi-node Bradian needs to know which replica owns which
shard, and a fencing token so a replica that lost
ownership stops serving stale writes. Full placement +
reconfig is Gate-sized.

## Decision

New module `bradian_distributed::shard_placement`:

```rust
pub struct ShardId(pub u64);
pub struct FencingToken(pub u64);
pub struct Placement { pub shard: ShardId, pub primary: ReplicaId, pub fence: FencingToken }
pub struct PlacementMap { ... }
impl PlacementMap {
    pub fn assign(&self, shard: ShardId, primary: ReplicaId) -> FencingToken;
    pub fn primary_of(&self, shard: ShardId) -> Option<Placement>;
    pub fn check_fence(&self, shard: ShardId, token: FencingToken) -> bool;
}
```

`assign` monotonically advances the fence for that shard;
`check_fence` returns `true` only if the caller's token
matches the current fence.

Honest scope: in-memory map, no durable manifest, no
reconfig protocol. Follow-ups will persist placements via
ADR 0309 consensus.

## Consequences

- 2 unit tests: assign returns monotonically-advancing
  fence; a stale fence is rejected by `check_fence`.
