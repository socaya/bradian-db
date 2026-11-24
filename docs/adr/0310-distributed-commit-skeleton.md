# ADR 0310 — Distributed commit + global snapshot: skeleton

## Status

Accepted as a skeleton. Third slice of P2.

## Context

A multi-shard commit needs an atomic "all-or-nothing"
protocol plus a global snapshot token so readers see a
consistent cross-shard view. Full 2PC / Calvin / Spanner
TrueTime is Gate-sized; this slice ships the typed surface.

## Decision

New module `bradian_distributed::distributed_commit`:

```rust
pub struct GlobalSnapshotToken(pub u64);
pub struct ShardCommitVote { pub shard: u64, pub ok: bool }
pub trait DistributedCommit {
    fn prepare(&self, shards: &[u64]) -> CommitResult<GlobalSnapshotToken>;
    fn commit (&self, token: GlobalSnapshotToken) -> CommitResult<()>;
    fn abort  (&self, token: GlobalSnapshotToken) -> CommitResult<()>;
}
pub struct InMemoryTwoPhaseCommit { ... }
```

Stub semantics: `prepare` returns a monotonically
advancing token, `commit` moves it to the committed set,
`abort` marks it aborted; each token exists in exactly
one terminal state.

Honest scope: **single-process**, no cross-replica
coordination, no participant failure handling.

## Consequences

- Downstream slices can take a `GlobalSnapshotToken` as a
  read parameter.
- 2 unit tests: prepare advances; commit after prepare
  succeeds; double-commit is rejected.
