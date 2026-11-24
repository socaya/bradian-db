# ADR 0309 — Consensus-backed replication: skeleton

## Status

Accepted as a skeleton. Second slice of P2.

## Context

Durable, highly-available Bradian needs a consensus-backed
log so each replica applies the same committed row
sequence. A full Raft / Paxos implementation is a
Gate-sized effort; this slice ships the **typed surface**
so later slices can fill in a real transport.

## Decision

New module `bradian_distributed::consensus` exposing:

```rust
pub struct ReplicaId(pub u64);
pub struct Term(pub u64);
pub struct LogIndex(pub u64);
pub enum Role { Follower, Candidate, Leader }

pub struct LogEntry { pub term: Term, pub index: LogIndex, pub bytes: Vec<u8> }

pub trait ConsensusGroup {
    fn append(&self, bytes: Vec<u8>) -> ConsensusResult<LogIndex>;
    fn committed_index(&self) -> LogIndex;
    fn role(&self) -> Role;
}

pub struct InMemoryConsensusGroup { ... }  // single-replica honest stub
```

Single-replica stub semantics:

- `append` assigns the next monotonic index and commits it
  immediately (one-replica majority).
- `role` is always `Leader`.
- `committed_index` returns the max assigned index.

Honest scope:

- **No peers, no election, no replication, no network.**
  A single-replica group is trivially consistent; the
  stub only exercises the typed surface + the monotonic
  index invariant.
- No log compaction, no snapshot transfer, no reconfig.
- Future slices will replace the backing with a real Raft
  state machine driven by `bradian-serving` over mTLS
  (ADR 0283/0284).

## Consequences

- P2 consumers (distributed commit 0310, shard placement
  0311) can reference `ConsensusGroup` by trait rather
  than waiting for a real implementation.
- 2 unit tests: append advances `committed_index`
  monotonically; role is always Leader on the stub.
- No production claim: the module docstring says
  "single-replica stub; not safe for multi-node use."
