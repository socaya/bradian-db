# ADR 0132 — PITR restore-plan selector (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch D D4 primitive —
completes Batch D. Companion to ADR 0102 (PITR map) and
ADR 0121 (incremental backup delta writer).

## 1. Scope

New module `bradian_relational::pitr_planner`:

- `plan_restore(target_lsn, bases, deltas) -> Result<RestorePlan>`
  — pure function. No I/O, no backup validation (assumes
  the descriptor slices came from a verified inventory).
- `BaseBackup { id, tip_lsn }` descriptor.
- `DeltaBackup { id, base_lsn, tip_lsn }` descriptor.
- `RestorePlan { base, deltas, target_lsn }` — base
  applied first, then each delta in order.
- `PlannerError` enum: `NoBase` / `ChainGap` /
  `TargetBeyondReach` / `MalformedDelta`.

Algorithm:
1. Validate every delta has `base_lsn ≤ tip_lsn`.
2. Pick the latest base with `tip_lsn ≤ target_lsn`.
3. If `base.tip_lsn == target_lsn`, return (no deltas).
4. Walk deltas chaining `base.tip_lsn → delta.tip_lsn`,
   picking the longest matching jump each step.
5. If cursor doesn't reach `target_lsn`, return
   `TargetBeyondReach { reachable }`.

## 2. Out of scope (follow-ups)

- **Executor / file I/O** — the planner is pure.
  Applying the plan touches the filesystem and is a
  separate ADR.
- **Backup integrity verification** — the caller must
  validate BACE receipts and CRCs before passing
  descriptors here.
- **Multi-tenant / per-shard planning** — single
  monotone LSN space assumed.
- **Delta compaction policy** — the planner just picks
  the longest jump; it does not advise whether short
  deltas should be merged.
- **Target between two deltas** — if
  `target_lsn` is strictly between two tip LSNs the
  planner reports `TargetBeyondReach` because the WAL
  tail replay to a mid-delta LSN is not yet
  implemented.
- **Auto-pruning** — stale base/delta cleanup is a
  separate operator concern.

## 3. Compliance

- Base exactly at target → no deltas.
- Latest-base-not-exceeding-target selected.
- Empty inventory → `NoBase`.
- Chain reaches target exactly.
- Chain gap → `TargetBeyondReach`.
- Deltas above target ignored.
- Multiple deltas from same cursor → longest tip picked.
- Malformed delta rejected before planning.
- Target matches base LSN → deltas from same LSN
  ignored.
- Target beyond chain → `TargetBeyondReach` with correct
  `reachable` field.
- 11 new tests in `pitr_planner::tests`.

## 4. Honest status

Foundation only. **BRD-DR-002 auto-PITR** moves from
PARTIAL to STILL-PARTIAL because the planner lives
alongside ADR 0102 (map) and ADR 0121 (delta writer) but
no executor glues them together. A future ADR applies
the plan to the filesystem (copy base, replay deltas,
truncate WAL to target_lsn).
