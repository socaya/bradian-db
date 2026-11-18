# ADR 0167 — PITR restore executor (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Completes Batch D
by shipping the executor side of ADR 0132's
`RestorePlan`. Combined with ADR 0102 (PITR map) and
ADR 0121 (incremental delta writer), operators now have
an end-to-end point-in-time recovery pipeline.

## 1. Scope

New module `pitr_executor`:

- `apply_plan(plan, source_dir, destination) -> u64`
  copies the plan's base backup directory into a fresh
  destination, then appends each delta's body to a
  `wal.replay` file in plan order.
- Returns bytes appended to `wal.replay`.
- Delta parsing reproduces ADR 0121's format (MAGIC +
  version + base_lsn + tip_lsn + body_len + body + CRC32)
  and validates CRC before accepting bytes.
- `ExecutorError` variants: `Io`,
  `DestinationExists`, `MissingBase`, `MissingDelta`,
  `MalformedDelta`.

Expected on-disk layout under `source_dir`:
```
source_dir/
  bases/<base_id>/...   (recursively copied)
  deltas/<id>.brdl      (parsed per delta)
```

## 2. Out of scope

- **Online restore** — the executor is offline;
  destination must not already exist.
- **Backup receipt re-verification** — caller
  verifies BACE receipts before building the plan.
- **Parallel delta copy** — sequential today.
- **Mid-delta WAL tail truncation** — the ADR 0132
  planner caps target_lsn at a delta tip.

## 3. Compliance

6 unit tests in `pitr_executor::tests`:

- `apply_plan_copies_base_and_appends_deltas`.
- `apply_plan_rejects_existing_destination`.
- `apply_plan_rejects_missing_base`.
- `apply_plan_rejects_missing_delta`.
- `apply_plan_detects_corrupted_delta`.
- `empty_delta_chain_only_copies_base`.

## 4. Honest status

**Batch D (Storage)** reaches 100%:
- D1 unique check (ADR 0101).
- D2 non-unique index helper (ADR 0127).
- D3 reader fuzz (ADR 0131).
- D4 PITR planner (ADR 0132) + **executor (this ADR)**.
