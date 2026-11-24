//! ADR 0309 — Consensus-backed replication **skeleton**.
//!
//! Typed surface only. The in-memory backing is a
//! single-replica stub: `append` returns a monotonic index
//! and treats it as committed. Not safe for multi-node use.

use parking_lot::Mutex;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ReplicaId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Term(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct LogIndex(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role { Follower, Candidate, Leader }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub term:  Term,
    pub index: LogIndex,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum ConsensusError {
    #[error("consensus group is stopped")]
    Stopped,
}

pub type ConsensusResult<T> = Result<T, ConsensusError>;

pub trait ConsensusGroup {
    fn append(&self, bytes: Vec<u8>) -> ConsensusResult<LogIndex>;
    fn committed_index(&self) -> LogIndex;
    fn role(&self) -> Role;
}

#[derive(Debug, Default)]
pub struct InMemoryConsensusGroup {
    log: Mutex<Vec<LogEntry>>,
}

impl InMemoryConsensusGroup {
    pub fn new() -> Self { Self::default() }
}

impl ConsensusGroup for InMemoryConsensusGroup {
    fn append(&self, bytes: Vec<u8>) -> ConsensusResult<LogIndex> {
        let mut log = self.log.lock();
        let index = LogIndex(log.len() as u64 + 1);
        log.push(LogEntry { term: Term(1), index, bytes });
        Ok(index)
    }
    fn committed_index(&self) -> LogIndex {
        LogIndex(self.log.lock().len() as u64)
    }
    fn role(&self) -> Role { Role::Leader }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_advances_committed_index_monotonically() {
        let g = InMemoryConsensusGroup::new();
        assert_eq!(g.committed_index(), LogIndex(0));
        let i1 = g.append(vec![1, 2, 3]).unwrap();
        let i2 = g.append(vec![4, 5, 6]).unwrap();
        assert_eq!(i1, LogIndex(1));
        assert_eq!(i2, LogIndex(2));
        assert_eq!(g.committed_index(), LogIndex(2));
    }

    #[test]
    fn single_replica_stub_is_always_leader() {
        let g = InMemoryConsensusGroup::new();
        assert_eq!(g.role(), Role::Leader);
        g.append(vec![0]).unwrap();
        assert_eq!(g.role(), Role::Leader);
    }
}
