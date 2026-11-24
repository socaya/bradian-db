//! ADR 0311 — Shard placement + fencing **skeleton**.
//!
//! In-memory map only; no durable manifest, no reconfig
//! protocol. Future slices will persist placements via
//! the consensus log (ADR 0309).

use std::collections::HashMap;

use parking_lot::Mutex;

use crate::consensus::ReplicaId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ShardId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct FencingToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub shard:   ShardId,
    pub primary: ReplicaId,
    pub fence:   FencingToken,
}

#[derive(Debug, Default)]
pub struct PlacementMap {
    inner: Mutex<HashMap<ShardId, Placement>>,
}

impl PlacementMap {
    pub fn new() -> Self { Self::default() }

    pub fn assign(&self, shard: ShardId, primary: ReplicaId) -> FencingToken {
        let mut g = self.inner.lock();
        let next_fence = FencingToken(
            g.get(&shard).map(|p| p.fence.0 + 1).unwrap_or(1));
        g.insert(shard, Placement { shard, primary, fence: next_fence });
        next_fence
    }

    pub fn primary_of(&self, shard: ShardId) -> Option<Placement> {
        self.inner.lock().get(&shard).copied()
    }

    pub fn check_fence(&self, shard: ShardId, token: FencingToken) -> bool {
        self.inner.lock().get(&shard).map(|p| p.fence == token).unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assign_returns_monotonically_advancing_fence_token() {
        let m = PlacementMap::new();
        let f1 = m.assign(ShardId(1), ReplicaId(10));
        assert_eq!(f1, FencingToken(1));
        let f2 = m.assign(ShardId(1), ReplicaId(11));
        assert_eq!(f2, FencingToken(2));
        let other = m.assign(ShardId(2), ReplicaId(20));
        assert_eq!(other, FencingToken(1),
            "fences are per-shard, not global");
    }

    #[test]
    fn stale_fence_is_rejected_by_check_fence() {
        let m = PlacementMap::new();
        let stale = m.assign(ShardId(1), ReplicaId(10));
        let fresh = m.assign(ShardId(1), ReplicaId(11));
        assert!(!m.check_fence(ShardId(1), stale));
        assert!( m.check_fence(ShardId(1), fresh));
        assert!(!m.check_fence(ShardId(2), fresh),
            "fence is only valid for its own shard");
    }
}
