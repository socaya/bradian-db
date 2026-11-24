# ADR 0312 — Shard rebalancing

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
durable rebalance coordinator** whose move state recovers
across a crash.

## Context

The skeleton exposed `RebalancePlan::diff` and
`apply_to` — pure planning on an in-memory map.  A
crashed coordinator mid-rebalance left no trail: an
operator restarting the process couldn't tell which shards
had moved and which still needed to.

## Decision

Add `RebalanceCoordinator`:

```rust
pub struct RebalanceCoordinator { ... }
impl RebalanceCoordinator {
    pub fn open(root: impl Into<PathBuf>) -> RebalanceResult<Self>;
    pub fn start_plan(&self, plan: &RebalancePlan) -> RebalanceResult<u64>;
    pub fn complete_move(&self, plan_id: u64, shard: ShardId,
                         placement: &PlacementMap) -> RebalanceResult<()>;
    pub fn progress(&self, plan_id: u64) -> Option<PlanProgress>;
    pub fn unfinished_plans(&self) -> Vec<PlanProgress>;
}
```

Backed by ADR 0309's `LocalConsensusLog` at
`{root}/rebalance.log`. Every record is a JSON
`RebalanceRecord`:

- `PlanStarted { plan_id, moves }`
- `MoveStarted { plan_id, shard, from, to }`
- `MoveCompleted { plan_id, shard }`
- `PlanFinished { plan_id }`

Semantics:

- `start_plan` appends `PlanStarted` then `MoveStarted`
  per move and returns the plan ID. Each append is
  `sync_data`'d (ADR 0309 contract).
- `complete_move` validates the move is in `Started`
  state (rejects unknown shard, already-completed shard,
  or already-finished plan), appends `MoveCompleted`,
  advances the placement map's primary/fence for that
  shard, and — if this was the last outstanding move —
  appends `PlanFinished`.
- `open(root)` replays the log; the in-memory state
  mirrors the log exactly.
- `unfinished_plans()` returns every plan not yet
  `PlanFinished`, with per-shard `MoveState`. An
  operator can see exactly which moves finished and
  which are still Started after a crash.

## Scope

**In scope:**
- Durable record of every plan + move lifecycle.
- Monotonic plan IDs across restart.
- Recovery of in-progress plans: `Completed` moves stay
  Completed, `Started`-without-`Completed` stays Started.
- Placement-map fence advancement on `complete_move`.
- 6 unit tests: original 2 (diff + apply) plus 4 new
  ones covering mid-rebalance restart recovery,
  double-complete rejection + unknown plan/shard
  rejection, monotonic-plan-ID across restart, and the
  edge case where an empty plan is NOT silently treated
  as finished.

**Out of scope** (dedicated follow-ups):
- Actual data movement (snapshot transfer, catch-up,
  handoff) — the surface is ready for a transfer layer
  to plug in between `start_plan` and `complete_move`.
- Rollback / abort of a plan mid-flight.
- Rebalance decision policy (which replica to move
  shards to); plans are supplied by the caller.

## Consequences

- An operator can restart a coordinator mid-rebalance
  and see exactly which moves finished.
- `MoveShard` is now `Serialize + Deserialize` for log
  round-trip.
- The 2026-11-24 "SKELETON" status on 0312 is retired.
