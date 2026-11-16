# ADR 0102 — PITR timestamp→LSN map (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16 as a lookup layer.**

## 1. Honest scope

`AS OF <lsn>` already exists through `HistorySqlEngine` /
`DurableMvccStore`. ADR 0102 ships the **operator-friendly
lookup layer** that converts wall-clock timestamps to
durable LSNs:

- `bradian_relational::pitr::PointInTimeMap`
- `record(Checkpoint { commit_time_ms, lsn })` — append-only
  with timestamp + LSN monotonicity checks.
- `lsn_as_of(target_ms) -> u64` — binary search returning
  the latest LSN with `commit_time_ms <= target`.
- `truncate_before(horizon_ms)` for retention policies.
- Serde round-trip for persistence.

## 2. Out of scope

- **Destructive WAL rewind / restore-to-point.** Historical
  reads use `AS OF LSN` on live data; no PITR restore that
  truncates WAL.
- **Automatic recording** from `commit_batch`. Callers (an
  engine, a supervisor) populate the map.
- **Durable storage** of the map. Callers serialise via
  serde if persistence is needed.
- **Clock-skew tolerance**. Timestamps must non-decrease.

## 3. Compliance

1. Empty map → `PitrError::Empty`.
2. `lsn_as_of` returns the latest-at-or-before.
3. Target before earliest → `TargetBeforeEarliest`.
4. Same millisecond allows multiple LSNs; returns the
   largest.
5. Regression (ts or lsn) rejected.
6. `truncate_before` preserves at least the latest entry.
7. Serde round-trip exact.
8. 7 unit tests in `src/pitr.rs::tests`.
