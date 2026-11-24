//! ADR 0310 — Distributed commit + global snapshot.
//!
//! Ships a **durable two-phase-commit coordinator**: every
//! `prepare` / `commit` / `abort` decision is appended to
//! a crash-safe log via the existing `LocalConsensusLog`
//! transport (ADR 0309), so the coordinator's state is
//! recoverable after restart. The coordinator is still
//! single-process — real cross-replica RPC to participants
//! remains a follow-up — but the **recoverability** of
//! the decision table is already production-shape.
//!
//! The in-memory `InMemoryTwoPhaseCommit` stays available
//! for tests that don't need durability.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::consensus::{ConsensusGroup, LocalConsensusLog};

type ReplayedState = (
    HashMap<GlobalSnapshotToken, CommitState>,
    HashMap<GlobalSnapshotToken, Vec<u64>>,
    u64,
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct GlobalSnapshotToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommitState { Prepared, Committed, Aborted }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShardCommitVote { pub shard: u64, pub ok: bool }

#[derive(Debug, Error)]
pub enum CommitError {
    #[error("unknown snapshot token")]
    Unknown,
    #[error("snapshot token is not in Prepared state")]
    WrongState,
    #[error("durable coordinator io: {0}")]
    Durable(#[from] DurableError),
}
pub type CommitResult<T> = Result<T, CommitError>;

#[derive(Debug, Error)]
pub enum DurableError {
    #[error("consensus log io: {0}")]
    Log(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("decision log references unknown prior token {0:?}")]
    OrphanDecision(GlobalSnapshotToken),
}

impl From<crate::consensus::ConsensusError> for DurableError {
    fn from(e: crate::consensus::ConsensusError) -> Self {
        DurableError::Log(format!("{e}"))
    }
}

pub trait DistributedCommit {
    fn prepare(&self, shards: &[u64])             -> CommitResult<GlobalSnapshotToken>;
    fn commit (&self, token: GlobalSnapshotToken) -> CommitResult<()>;
    fn abort  (&self, token: GlobalSnapshotToken) -> CommitResult<()>;
}

/// In-memory coordinator — no disk, no recovery.
#[derive(Debug, Default)]
pub struct InMemoryTwoPhaseCommit {
    next:   Mutex<u64>,
    states: Mutex<HashMap<GlobalSnapshotToken, CommitState>>,
}

impl InMemoryTwoPhaseCommit {
    pub fn new() -> Self { Self::default() }
    pub fn state(&self, t: GlobalSnapshotToken) -> Option<CommitState> {
        self.states.lock().get(&t).copied()
    }
}

impl DistributedCommit for InMemoryTwoPhaseCommit {
    fn prepare(&self, _shards: &[u64]) -> CommitResult<GlobalSnapshotToken> {
        let mut n = self.next.lock();
        *n += 1;
        let token = GlobalSnapshotToken(*n);
        self.states.lock().insert(token, CommitState::Prepared);
        Ok(token)
    }
    fn commit(&self, token: GlobalSnapshotToken) -> CommitResult<()> {
        let mut s = self.states.lock();
        match s.get(&token) {
            None => Err(CommitError::Unknown),
            Some(CommitState::Prepared) => { s.insert(token, CommitState::Committed); Ok(()) }
            _ => Err(CommitError::WrongState),
        }
    }
    fn abort(&self, token: GlobalSnapshotToken) -> CommitResult<()> {
        let mut s = self.states.lock();
        match s.get(&token) {
            None => Err(CommitError::Unknown),
            Some(CommitState::Prepared) => { s.insert(token, CommitState::Aborted); Ok(()) }
            _ => Err(CommitError::WrongState),
        }
    }
}

// ---------------------------------------------------------------------------
// DurableTwoPhaseCommit — crash-safe decision log.

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum DecisionRecord {
    Prepare { token: GlobalSnapshotToken, shards: Vec<u64> },
    Commit  { token: GlobalSnapshotToken },
    Abort   { token: GlobalSnapshotToken },
}

/// Durable 2PC coordinator. Each decision is appended as
/// a JSON record to a `LocalConsensusLog`; the log's
/// `sync_data`-per-append makes the decision durable
/// before the call returns. On open, the log is replayed
/// to rebuild the in-memory decision table.
///
/// Recovery contract: after restart, a `Prepared` record
/// without a subsequent `Commit` or `Abort` STAYS Prepared.
/// That's the correct 2PC behaviour — the operator (or a
/// higher-level reconcile loop) must decide. The coordinator
/// does not unilaterally convert Prepared to Aborted.
#[derive(Debug)]
pub struct DurableTwoPhaseCommit {
    log:         std::sync::Arc<LocalConsensusLog>,
    next_id:     AtomicU64,
    state:       Mutex<HashMap<GlobalSnapshotToken, CommitState>>,
    prepared_shards: Mutex<HashMap<GlobalSnapshotToken, Vec<u64>>>,
}

impl DurableTwoPhaseCommit {
    /// Open or create the coordinator at `root`. Replays
    /// the decision log so restart-recovery observes every
    /// prior Prepared / Committed / Aborted record.
    pub fn open(root: impl Into<PathBuf>) -> CommitResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e|
            CommitError::Durable(DurableError::Log(format!("mkdir: {e}"))))?;
        let log = std::sync::Arc::new(
            LocalConsensusLog::open(root.join("twopc.log"))
                .map_err(DurableError::from)?);
        let (state, prepared_shards, max_token) = Self::replay(log.as_ref())?;
        Ok(Self {
            log, next_id: AtomicU64::new(max_token),
            state:           Mutex::new(state),
            prepared_shards: Mutex::new(prepared_shards),
        })
    }

    fn replay(log: &LocalConsensusLog) -> CommitResult<ReplayedState> {
        let mut state   = HashMap::new();
        let mut prepared = HashMap::new();
        let mut max     = 0u64;
        for i in 1..=log.committed_index().0 {
            let entry = log.entry(crate::consensus::LogIndex(i))
                .expect("entry must exist for in-range index");
            let rec: DecisionRecord = serde_json::from_slice(&entry.bytes)
                .map_err(DurableError::from)?;
            match rec {
                DecisionRecord::Prepare { token, shards } => {
                    state.insert(token, CommitState::Prepared);
                    prepared.insert(token, shards);
                    if token.0 > max { max = token.0; }
                }
                DecisionRecord::Commit { token } => {
                    match state.get(&token) {
                        Some(CommitState::Prepared) =>
                            { state.insert(token, CommitState::Committed); }
                        _ => return Err(CommitError::Durable(
                                DurableError::OrphanDecision(token))),
                    }
                }
                DecisionRecord::Abort { token } => {
                    match state.get(&token) {
                        Some(CommitState::Prepared) =>
                            { state.insert(token, CommitState::Aborted); }
                        _ => return Err(CommitError::Durable(
                                DurableError::OrphanDecision(token))),
                    }
                }
            }
        }
        Ok((state, prepared, max))
    }

    pub fn state(&self, token: GlobalSnapshotToken) -> Option<CommitState> {
        self.state.lock().get(&token).copied()
    }

    pub fn prepared_shards(&self, token: GlobalSnapshotToken) -> Option<Vec<u64>> {
        self.prepared_shards.lock().get(&token).cloned()
    }

    pub fn unresolved_tokens(&self) -> Vec<GlobalSnapshotToken> {
        self.state.lock().iter()
            .filter_map(|(t, s)| if *s == CommitState::Prepared { Some(*t) } else { None })
            .collect()
    }

    fn append(&self, record: DecisionRecord) -> CommitResult<()> {
        let bytes = serde_json::to_vec(&record).map_err(DurableError::from)?;
        self.log.append(bytes).map_err(|e| CommitError::Durable(DurableError::Log(format!("{e}"))))?;
        Ok(())
    }
}

impl DistributedCommit for DurableTwoPhaseCommit {
    fn prepare(&self, shards: &[u64]) -> CommitResult<GlobalSnapshotToken> {
        let token = GlobalSnapshotToken(self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
        self.append(DecisionRecord::Prepare { token, shards: shards.to_vec() })?;
        self.state.lock().insert(token, CommitState::Prepared);
        self.prepared_shards.lock().insert(token, shards.to_vec());
        Ok(token)
    }

    fn commit(&self, token: GlobalSnapshotToken) -> CommitResult<()> {
        {
            let s = self.state.lock();
            match s.get(&token) {
                None                        => return Err(CommitError::Unknown),
                Some(CommitState::Prepared) => {}
                _                           => return Err(CommitError::WrongState),
            }
        }
        self.append(DecisionRecord::Commit { token })?;
        self.state.lock().insert(token, CommitState::Committed);
        Ok(())
    }

    fn abort(&self, token: GlobalSnapshotToken) -> CommitResult<()> {
        {
            let s = self.state.lock();
            match s.get(&token) {
                None                        => return Err(CommitError::Unknown),
                Some(CommitState::Prepared) => {}
                _                           => return Err(CommitError::WrongState),
            }
        }
        self.append(DecisionRecord::Abort { token })?;
        self.state.lock().insert(token, CommitState::Aborted);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_then_commit_advances_through_states() {
        let c = InMemoryTwoPhaseCommit::new();
        let t = c.prepare(&[1, 2, 3]).unwrap();
        assert_eq!(c.state(t), Some(CommitState::Prepared));
        c.commit(t).unwrap();
        assert_eq!(c.state(t), Some(CommitState::Committed));
    }

    #[test]
    fn double_commit_and_commit_after_abort_are_rejected() {
        let c = InMemoryTwoPhaseCommit::new();
        let t = c.prepare(&[]).unwrap();
        c.commit(t).unwrap();
        assert!(matches!(c.commit(t), Err(CommitError::WrongState)));
        let t2 = c.prepare(&[]).unwrap();
        c.abort(t2).unwrap();
        assert!(matches!(c.commit(t2), Err(CommitError::WrongState)));
    }

    #[test]
    fn unknown_token_is_rejected() {
        let c = InMemoryTwoPhaseCommit::new();
        assert!(matches!(c.commit(GlobalSnapshotToken(99)), Err(CommitError::Unknown)));
    }

    #[test]
    fn durable_coordinator_recovers_all_decisions_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        let (t_prepared, t_committed, t_aborted) = {
            let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
            let a = c.prepare(&[1, 2]).unwrap();
            let b = c.prepare(&[3]).unwrap();
            let x = c.prepare(&[]).unwrap();
            c.commit(b).unwrap();
            c.abort (x).unwrap();
            (a, b, x)
        };
        let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
        assert_eq!(c.state(t_prepared), Some(CommitState::Prepared));
        assert_eq!(c.state(t_committed), Some(CommitState::Committed));
        assert_eq!(c.state(t_aborted),  Some(CommitState::Aborted));
        assert_eq!(c.prepared_shards(t_prepared), Some(vec![1, 2]));
        // The one still Prepared is visible as unresolved
        // so a reconcile loop can finish it.
        let unresolved = c.unresolved_tokens();
        assert_eq!(unresolved, vec![t_prepared]);
    }

    #[test]
    fn next_prepare_after_reopen_uses_a_fresh_monotonic_id() {
        let dir = tempfile::tempdir().unwrap();
        let t1 = {
            let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
            c.prepare(&[1]).unwrap();
            c.prepare(&[2]).unwrap()
        };
        let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
        let t3 = c.prepare(&[3]).unwrap();
        assert!(t3.0 > t1.0,
            "post-restart prepare must advance past the persisted max");
    }

    #[test]
    fn commit_after_abort_still_rejected_on_durable_coordinator() {
        let dir = tempfile::tempdir().unwrap();
        let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
        let t = c.prepare(&[]).unwrap();
        c.abort(t).unwrap();
        let err = c.commit(t).unwrap_err();
        assert!(matches!(err, CommitError::WrongState));
    }

    #[test]
    fn unknown_token_rejected_on_durable_coordinator() {
        let dir = tempfile::tempdir().unwrap();
        let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
        let err = c.commit(GlobalSnapshotToken(7)).unwrap_err();
        assert!(matches!(err, CommitError::Unknown));
    }

    #[test]
    fn prepared_without_decision_stays_prepared_across_restart() {
        // The core 2PC recovery contract: a Prepared
        // token without Commit/Abort must NOT be silently
        // converted to Aborted on restart. The operator
        // (or a higher-level reconciler) decides.
        let dir = tempfile::tempdir().unwrap();
        let token = {
            let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
            c.prepare(&[42, 43]).unwrap()
        };
        let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
        assert_eq!(c.state(token), Some(CommitState::Prepared));
        // And a committed resolve still works after restart.
        c.commit(token).unwrap();
        assert_eq!(c.state(token), Some(CommitState::Committed));
        // Reopening once more sees the Commit.
        let c = DurableTwoPhaseCommit::open(dir.path()).unwrap();
        assert_eq!(c.state(token), Some(CommitState::Committed));
    }
}
