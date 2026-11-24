# ADR 0310 — Distributed commit + global snapshot

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
durable two-phase-commit coordinator** whose decision
table is recoverable after crash.

## Context

The skeleton exposed `InMemoryTwoPhaseCommit` whose
`prepare` / `commit` / `abort` state lived only in
process memory. A real distributed commit needs the
decision to be durably persisted BEFORE the call returns
so that the coordinator can recover from crash without
losing an acknowledged commit decision.

## Decision

Two implementations of `DistributedCommit`:

```rust
pub struct InMemoryTwoPhaseCommit { ... }   // unchanged — tests
pub struct DurableTwoPhaseCommit  { ... }   // persistent decision log
```

`DurableTwoPhaseCommit`:

- Backed by a `LocalConsensusLog` at `{root}/twopc.log`
  (ADR 0309's durable log: length-prefixed CRC32 records,
  `sync_data` per append, torn-tail trimming on reopen).
- Each decision is appended as a JSON `DecisionRecord`
  (`Prepare { token, shards } | Commit { token } |
  Abort { token }`) BEFORE the call returns.
- `open(root)` replays the log, rebuilding the
  `token → CommitState` table and the
  `token → shards` table. An orphan Commit/Abort (no
  prior Prepare) is surfaced as corruption.
- `unresolved_tokens()` returns every Prepared token
  that has no subsequent decision — the primary read
  for an operator reconciler.

Recovery contract (the 2PC-canonical part):

- A Prepared record without Commit/Abort **stays
  Prepared** after restart. The coordinator does NOT
  unilaterally convert to Abort; the operator (or a
  higher-level reconciler consulting participant state)
  decides.
- Prepare-IDs monotonically advance across restart —
  `next_id` is seeded from the max replayed token.

## Scope

**In scope:**
- Durable Prepare / Commit / Abort via ADR 0309's log.
- Replay on open; monotonic next-ID across restart;
  per-token shard list recovery.
- Typed `DurableError` surface distinguishing log IO,
  serde, and orphan-decision bugs.
- 8 unit tests: 3 preserved from the skeleton (in-memory
  state transitions) + 5 new ones covering
  decisions-recover-across-restart with every state
  combination, monotonic-ID-after-restart, commit-after-
  abort rejection on durable, unknown-token rejection on
  durable, and the canonical "Prepared-stays-Prepared"
  recovery contract.

**Out of scope** (dedicated follow-ups):
- Real participant RPC (two-phase prepare + ack vote
  exchange). The coordinator still assumes participants
  are co-process.
- Timeout-driven unilateral abort.
- Snapshot epoch / global visibility rules (`CommitState`
  here is just a 3-state enum).

## Consequences

- An operator can crash + restart the coordinator and
  get the identical decision table back, with any
  Prepared-but-unresolved token visible for reconcile.
- 2PC's historical foot-gun — losing decisions on crash
  — is fixed for the decision-log layer.
- The 2026-11-24 "SKELETON" status on 0310 is retired.
