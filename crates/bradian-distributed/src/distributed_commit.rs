//! ADR 0310 — Distributed commit + global snapshot **skeleton**.
//!
//! Single-process typed stub. No cross-replica
//! coordination, no participant failure handling.

use std::collections::HashMap;

use parking_lot::Mutex;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct GlobalSnapshotToken(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitState { Prepared, Committed, Aborted }

#[derive(Debug, Error)]
pub enum CommitError {
    #[error("unknown snapshot token")]
    Unknown,
    #[error("snapshot token is not in Prepared state")]
    WrongState,
}

pub type CommitResult<T> = Result<T, CommitError>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShardCommitVote { pub shard: u64, pub ok: bool }

pub trait DistributedCommit {
    fn prepare(&self, shards: &[u64])            -> CommitResult<GlobalSnapshotToken>;
    fn commit (&self, token: GlobalSnapshotToken) -> CommitResult<()>;
    fn abort  (&self, token: GlobalSnapshotToken) -> CommitResult<()>;
}

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
}
