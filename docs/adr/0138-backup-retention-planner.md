# ADR 0138 — Backup retention policy planner (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Companion to:
- ADR 0015 base-backup receipts
- ADR 0121 incremental-backup delta writer
- ADR 0132 PITR restore-plan selector

## 1. Scope

New module `bradian_relational::backup_retention`:

- `RetentionPolicy { min_keep_bases, min_recoverable_lsn,
  max_age_days }` — three knobs.
- `AgedBase { base: BaseBackup, age_days }` and
  `AgedDelta { delta: DeltaBackup, age_days }` wrap the
  ADR 0132 descriptors with age.
- `plan_retention(bases, deltas, policy) -> RetentionPlan`
  — pure function.
- `RetentionPlan { keep_bases, prune_bases, keep_deltas,
  prune_deltas }` — four partitioned Vec<String> lists.

Algorithm (in order):
1. Hard floor: always keep the latest `min_keep_bases.max(1)`
   bases.
2. Coverage pin: if `min_recoverable_lsn` is set, keep the
   latest base with `tip_lsn <= target`.
3. Age pruning: bases older than `max_age_days` are
   eligible for pruning (unless pinned by rules 1-2).
4. Delta chain: iteratively extend the "kept tip LSN" set
   from kept bases; deltas whose base_lsn isn't in the
   set are orphans. If a coverage pin is set, deltas with
   `tip_lsn > target` are pruned.

## 2. Out of scope (follow-ups)

- **Filesystem execution** — caller deletes files.
- **Encrypted-backup key rotation** — retention, not
  key-management.
- **Cross-tenant / per-database retention** — one
  global inventory.
- **Automatic scheduled runs** — an operator / cron
  calls the planner.
- **Wall-clock age source** — `age_days` is caller-
  supplied; a future primitive can map receipt
  timestamps.

## 3. Compliance

- `min_keep_bases` preserves the latest N bases.
- Floor of 1 enforced even if 0 requested.
- `min_recoverable_lsn` pins the covering base.
- Orphan deltas (base_lsn not matching any base tip)
  are pruned.
- Deltas pointing at a pruned base are pruned.
- Deltas beyond a coverage pin are pruned.
- Default policy keeps everything.
- Empty inventory yields empty plan.
- Chain gap: everything past the gap is pruned.
- 9 new tests in `backup_retention::tests`.

## 4. Honest status

Foundation only. **BRD-DR-003 backup lifecycle** is not
yet satisfied because no filesystem executor applies
plans, no encrypted-backup key-rotation integration
exists, and no scheduled-run coordinator ships. The
planner is the prerequisite that a future operator
tool consumes.
