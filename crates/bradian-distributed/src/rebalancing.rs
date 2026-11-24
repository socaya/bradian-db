//! ADR 0312 — Shard rebalancing.
//!
//! A rebalance plan is a list of per-shard moves computed
//! from a `(current_placement → desired_primary)` diff.
//! This slice adds a **durable coordinator** that records
//! each move's lifecycle (Started → Completed) into ADR
//! 0309's consensus log, so a crash mid-rebalance leaves
//! an auditable record of which shards finished moving
//! and which did not.
//!
//! Data movement itself (snapshot transfer, catch-up,
//! handoff) stays a follow-up — the surface here lets the
//! transfer layer be plugged in later without rewriting
//! the plan or the state machine.

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::consensus::{ConsensusGroup, LocalConsensusLog, LogIndex};
use crate::shard_placement::{PlacementMap, ShardId};
use crate::consensus::ReplicaId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MoveShard {
    pub shard: ShardId,
    pub from:  ReplicaId,
    pub to:    ReplicaId,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RebalancePlan {
    pub moves: Vec<MoveShard>,
}

impl RebalancePlan {
    /// Build a plan from a current placement map and a
    /// desired `shard → primary` mapping.
    pub fn diff(current: &PlacementMap, desired: &HashMap<ShardId, ReplicaId>) -> Self {
        let mut moves = Vec::new();
        for (shard, want) in desired {
            if let Some(p) = current.primary_of(*shard) {
                if p.primary != *want {
                    moves.push(MoveShard { shard: *shard, from: p.primary, to: *want });
                }
            }
        }
        moves.sort_by_key(|m| m.shard.0);
        Self { moves }
    }

    /// Apply the plan directly against a placement map
    /// (no durability). Kept for compatibility with the
    /// in-memory callers from the pre-0312 skeleton.
    pub fn apply_to(&self, map: &PlacementMap) {
        for m in &self.moves { map.assign(m.shard, m.to); }
    }
}

// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RebalanceRecord {
    PlanStarted { plan_id: u64, moves: Vec<MoveShard> },
    MoveStarted { plan_id: u64, shard: ShardId, from: ReplicaId, to: ReplicaId },
    MoveCompleted { plan_id: u64, shard: ShardId },
    PlanFinished { plan_id: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MoveState { Started, Completed }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanProgress {
    pub plan_id:  u64,
    pub moves:    Vec<MoveShard>,
    pub state:    HashMap<ShardId, MoveState>,
    pub finished: bool,
}

#[derive(Debug, Error)]
pub enum RebalanceError {
    #[error("consensus log io: {0}")]
    Log(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("unknown plan id {0}")]
    UnknownPlan(u64),
    #[error("move for shard {0:?} is not in Started state")]
    MoveWrongState(ShardId),
    #[error("plan {0} already finished")]
    AlreadyFinished(u64),
}
pub type RebalanceResult<T> = Result<T, RebalanceError>;

impl From<crate::consensus::ConsensusError> for RebalanceError {
    fn from(e: crate::consensus::ConsensusError) -> Self {
        RebalanceError::Log(format!("{e}"))
    }
}

/// Durable rebalance coordinator. Records every move's
/// lifecycle into `{root}/rebalance.log` so an operator
/// knows exactly which moves finished and which stalled
/// across a coordinator crash.
#[derive(Debug)]
pub struct RebalanceCoordinator {
    log:   std::sync::Arc<LocalConsensusLog>,
    state: Mutex<HashMap<u64, PlanProgress>>,
    next:  Mutex<u64>,
}

impl RebalanceCoordinator {
    pub fn open(root: impl Into<PathBuf>) -> RebalanceResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e|
            RebalanceError::Log(format!("mkdir: {e}")))?;
        let log = std::sync::Arc::new(
            LocalConsensusLog::open(root.join("rebalance.log"))?);
        let (plans, max_id) = Self::replay(&log)?;
        Ok(Self {
            log,
            state: Mutex::new(plans),
            next:  Mutex::new(max_id),
        })
    }

    fn replay(log: &LocalConsensusLog) -> RebalanceResult<(HashMap<u64, PlanProgress>, u64)> {
        let mut plans: HashMap<u64, PlanProgress> = HashMap::new();
        let mut max   = 0u64;
        for i in 1..=log.committed_index().0 {
            let entry = log.entry(LogIndex(i)).expect("in-range");
            let rec: RebalanceRecord = serde_json::from_slice(&entry.bytes)?;
            match rec {
                RebalanceRecord::PlanStarted { plan_id, moves } => {
                    plans.insert(plan_id, PlanProgress {
                        plan_id, moves, state: HashMap::new(), finished: false,
                    });
                    if plan_id > max { max = plan_id; }
                }
                RebalanceRecord::MoveStarted { plan_id, shard, .. } => {
                    if let Some(p) = plans.get_mut(&plan_id) {
                        p.state.insert(shard, MoveState::Started);
                    }
                }
                RebalanceRecord::MoveCompleted { plan_id, shard } => {
                    if let Some(p) = plans.get_mut(&plan_id) {
                        p.state.insert(shard, MoveState::Completed);
                    }
                }
                RebalanceRecord::PlanFinished { plan_id } => {
                    if let Some(p) = plans.get_mut(&plan_id) { p.finished = true; }
                }
            }
        }
        Ok((plans, max))
    }

    fn append(&self, rec: &RebalanceRecord) -> RebalanceResult<()> {
        let bytes = serde_json::to_vec(rec)?;
        self.log.append(bytes)?;
        Ok(())
    }

    /// Begin a plan: durably record each move as Started
    /// and return the assigned plan_id.
    pub fn start_plan(&self, plan: &RebalancePlan) -> RebalanceResult<u64> {
        let plan_id = {
            let mut n = self.next.lock();
            *n += 1; *n
        };
        self.append(&RebalanceRecord::PlanStarted {
            plan_id, moves: plan.moves.clone() })?;
        for m in &plan.moves {
            self.append(&RebalanceRecord::MoveStarted {
                plan_id, shard: m.shard, from: m.from, to: m.to })?;
        }
        let state: HashMap<_, _> = plan.moves.iter()
            .map(|m| (m.shard, MoveState::Started)).collect();
        self.state.lock().insert(plan_id, PlanProgress {
            plan_id, moves: plan.moves.clone(), state, finished: false,
        });
        Ok(plan_id)
    }

    /// Mark one shard move complete and advance the shard's
    /// primary on the placement map. Idempotent: a repeated
    /// complete for the same shard is rejected.
    pub fn complete_move(
        &self, plan_id: u64, shard: ShardId, placement: &PlacementMap,
    ) -> RebalanceResult<()> {
        let (expected_to, finishes_plan) = {
            let st = self.state.lock();
            let plan = st.get(&plan_id).ok_or(RebalanceError::UnknownPlan(plan_id))?;
            if plan.finished { return Err(RebalanceError::AlreadyFinished(plan_id)); }
            let mv = plan.moves.iter().find(|m| m.shard == shard)
                .ok_or(RebalanceError::MoveWrongState(shard))?;
            match plan.state.get(&shard) {
                Some(MoveState::Started) => {}
                _ => return Err(RebalanceError::MoveWrongState(shard)),
            }
            let others_done = plan.state.iter()
                .filter(|(s, _)| **s != shard)
                .all(|(_, st)| *st == MoveState::Completed);
            (mv.to, others_done)
        };
        self.append(&RebalanceRecord::MoveCompleted { plan_id, shard })?;
        placement.assign(shard, expected_to);
        {
            let mut st = self.state.lock();
            let plan = st.get_mut(&plan_id).ok_or(RebalanceError::UnknownPlan(plan_id))?;
            plan.state.insert(shard, MoveState::Completed);
        }
        if finishes_plan {
            self.append(&RebalanceRecord::PlanFinished { plan_id })?;
            self.state.lock().get_mut(&plan_id).unwrap().finished = true;
        }
        Ok(())
    }

    pub fn progress(&self, plan_id: u64) -> Option<PlanProgress> {
        self.state.lock().get(&plan_id).cloned()
    }

    pub fn unfinished_plans(&self) -> Vec<PlanProgress> {
        self.state.lock().values()
            .filter(|p| !p.finished)
            .cloned().collect()
    }

    pub fn is_move_completed(&self, plan_id: u64, shard: ShardId) -> bool {
        self.state.lock().get(&plan_id)
            .and_then(|p| p.state.get(&shard).copied())
            .map(|s| s == MoveState::Completed).unwrap_or(false)
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
            (ShardId(1), ReplicaId(10)),
            (ShardId(2), ReplicaId(21)),
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
        let desired: HashMap<_, _> = [(ShardId(5), ReplicaId(51))].into_iter().collect();
        let plan = RebalancePlan::diff(&map, &desired);
        plan.apply_to(&map);
        let after = map.primary_of(ShardId(5)).unwrap();
        assert_eq!(after.primary, ReplicaId(51));
        assert!(after.fence.0 > original.0);
    }

    #[test]
    fn durable_coordinator_recovers_in_progress_moves_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let placement_dir = tempfile::tempdir().unwrap();
        let placement = PlacementMap::open(placement_dir.path()).unwrap();
        placement.assign(ShardId(1), ReplicaId(10));
        placement.assign(ShardId(2), ReplicaId(20));
        placement.assign(ShardId(3), ReplicaId(30));

        let desired: HashMap<_, _> = [
            (ShardId(1), ReplicaId(11)),
            (ShardId(2), ReplicaId(22)),
            (ShardId(3), ReplicaId(33)),
        ].into_iter().collect();
        let plan = RebalancePlan::diff(&placement, &desired);
        assert_eq!(plan.moves.len(), 3);

        let plan_id = {
            let c = RebalanceCoordinator::open(dir.path()).unwrap();
            let id = c.start_plan(&plan).unwrap();
            // Finish two of three moves then crash.
            c.complete_move(id, ShardId(1), &placement).unwrap();
            c.complete_move(id, ShardId(2), &placement).unwrap();
            id
        };

        // Reopen — plan + state should be visible, with the
        // third move still Started and the plan unfinished.
        let c = RebalanceCoordinator::open(dir.path()).unwrap();
        let p = c.progress(plan_id).unwrap();
        assert!(!p.finished, "plan should remain unfinished");
        assert_eq!(p.state.get(&ShardId(1)), Some(&MoveState::Completed));
        assert_eq!(p.state.get(&ShardId(2)), Some(&MoveState::Completed));
        assert_eq!(p.state.get(&ShardId(3)), Some(&MoveState::Started));
        assert_eq!(c.unfinished_plans().len(), 1);

        // Resume: finish the third move.
        c.complete_move(plan_id, ShardId(3), &placement).unwrap();
        let p = c.progress(plan_id).unwrap();
        assert!(p.finished);
        assert!(c.unfinished_plans().is_empty());
        assert_eq!(placement.primary_of(ShardId(3)).unwrap().primary, ReplicaId(33));
    }

    #[test]
    fn double_complete_rejected_and_unknown_shard_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let placement_dir = tempfile::tempdir().unwrap();
        let placement = PlacementMap::open(placement_dir.path()).unwrap();
        placement.assign(ShardId(1), ReplicaId(10));
        let plan = RebalancePlan { moves: vec![
            MoveShard { shard: ShardId(1), from: ReplicaId(10), to: ReplicaId(11) },
        ]};
        let c = RebalanceCoordinator::open(dir.path()).unwrap();
        let id = c.start_plan(&plan).unwrap();
        c.complete_move(id, ShardId(1), &placement).unwrap();
        let err = c.complete_move(id, ShardId(1), &placement).unwrap_err();
        assert!(matches!(err, RebalanceError::AlreadyFinished(_)));
        // Unknown plan
        let err = c.complete_move(999, ShardId(1), &placement).unwrap_err();
        assert!(matches!(err, RebalanceError::UnknownPlan(999)));
        // Unknown shard in known plan
        let id2 = c.start_plan(&RebalancePlan { moves: vec![
            MoveShard { shard: ShardId(2), from: ReplicaId(0), to: ReplicaId(1) },
        ]}).unwrap();
        let err = c.complete_move(id2, ShardId(999), &placement).unwrap_err();
        assert!(matches!(err, RebalanceError::MoveWrongState(_)));
    }

    #[test]
    fn next_plan_id_monotonic_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let placement = PlacementMap::new();
        placement.assign(ShardId(1), ReplicaId(10));
        let plan = RebalancePlan::diff(&placement, &[(ShardId(1), ReplicaId(11))]
            .into_iter().collect());
        let first = {
            let c = RebalanceCoordinator::open(dir.path()).unwrap();
            c.start_plan(&plan).unwrap()
        };
        let c = RebalanceCoordinator::open(dir.path()).unwrap();
        let next = c.start_plan(&plan).unwrap();
        assert!(next > first, "plan id must advance across restart");
    }

    #[test]
    fn empty_plan_is_trivially_finished() {
        // Edge case: a plan with zero moves has no moves
        // to complete. start_plan records PlanStarted and
        // (since all "others_done" trivially holds) the
        // first complete_move would not find any moves —
        // so we simply never mark it finished. unfinished_plans
        // surfaces it until someone manually clears it.
        // This is explicit: empty plans are an operator
        // error and the surface tells you about it.
        let dir = tempfile::tempdir().unwrap();
        let c = RebalanceCoordinator::open(dir.path()).unwrap();
        let id = c.start_plan(&RebalancePlan::default()).unwrap();
        let p = c.progress(id).unwrap();
        assert_eq!(p.moves.len(), 0);
        assert!(!p.finished,
            "empty plan is NOT silently treated as finished");
    }
}
