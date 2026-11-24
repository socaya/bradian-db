//! ADR 0312 — Shard rebalancing **skeleton**.
//!
//! Pure planning + in-memory apply. No data movement, no
//! catch-up, no handoff.

use std::collections::HashMap;

use crate::consensus::ReplicaId;
use crate::shard_placement::{PlacementMap, ShardId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveShard {
    pub shard: ShardId,
    pub from:  ReplicaId,
    pub to:    ReplicaId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RebalancePlan {
    pub moves: Vec<MoveShard>,
}

impl RebalancePlan {
    /// Build a plan from a current placement map and a
    /// desired `shard → primary` mapping.
    pub fn diff(current: &PlacementMap, desired: &HashMap<ShardId, ReplicaId>) -> Self {
        let mut moves = Vec::new();
        for (shard, want) in desired {
            let have = current.primary_of(*shard);
            match have {
                Some(p) if p.primary != *want =>
                    moves.push(MoveShard { shard: *shard, from: p.primary, to: *want }),
                _ => {}
            }
        }
        moves.sort_by_key(|m| m.shard.0);
        Self { moves }
    }

    /// Apply the plan by reassigning each moved shard on
    /// the placement map (advances fence per shard).
    pub fn apply_to(&self, map: &PlacementMap) {
        for m in &self.moves { map.assign(m.shard, m.to); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_includes_only_shards_whose_primary_differs() {
        let map = PlacementMap::new();
        map.assign(ShardId(1), ReplicaId(10));
        map.assign(ShardId(2), ReplicaId(20));
        let desired: HashMap<ShardId, ReplicaId> = [
            (ShardId(1), ReplicaId(10)),   // unchanged
            (ShardId(2), ReplicaId(21)),   // move
        ].into_iter().collect();
        let plan = RebalancePlan::diff(&map, &desired);
        assert_eq!(plan.moves, vec![
            MoveShard { shard: ShardId(2), from: ReplicaId(20), to: ReplicaId(21) },
        ]);
    }

    #[test]
    fn apply_advances_fence_on_each_moved_shard() {
        let map = PlacementMap::new();
        map.assign(ShardId(5), ReplicaId(50));
        let original = map.primary_of(ShardId(5)).unwrap().fence;
        let desired: HashMap<ShardId, ReplicaId> = [(ShardId(5), ReplicaId(51))]
            .into_iter().collect();
        let plan = RebalancePlan::diff(&map, &desired);
        plan.apply_to(&map);
        let after = map.primary_of(ShardId(5)).unwrap();
        assert_eq!(after.primary, ReplicaId(51));
        assert!(after.fence.0 > original.0,
            "rebalance must advance the fence on move");
    }
}
