# ADR 0264 — Mutex-held reconcile + execute

## Status

Accepted.

## Context

ADR 0256's `execute_with_schema_snapshot` called `reconcile`
and then `execute` as two unlocked steps. A concurrent ALTER
could land in the race window between them, letting a SELECT
observe a mix of old and new schema states.

## Decision

- `SqlEngine.operation` upgrades from `parking_lot::Mutex<()>`
  to `parking_lot::ReentrantMutex<()>`. Reentrancy allows the
  outer `execute_with_schema_snapshot` to hold the guard
  across both reconcile and the inner `execute`, which also
  takes the same lock.
- `execute_with_schema_snapshot` now acquires the lock
  BEFORE the catalog snapshot read, then reconciles, then
  delegates to `execute`. The lock is released only after
  dispatch completes.

## Consequences

- No SQL path can land an ALTER between reconcile and execute.
  A concurrent ALTER thread blocks until the snapshot-protected
  statement releases the lock. Either the ALTER runs before
  reconcile (SELECT returns pre-state) or after execute
  (SELECT returns pre-state). The race window is closed.
- Other engines (`DomainSqlEngine`, `MvccSqlEngine`,
  `HistorySqlEngine`) use their own operation mutexes; ADRs
  0266 / 0267 integrate the snapshot there with the same
  pattern.
- `parking_lot::ReentrantMutex` has a small performance
  penalty vs. `Mutex`. Benchmarks are a future slice.
- 10 cases in `tests/schema_snapshot_execute.rs` cover the
  existing 9 ADR 0256 cases plus a new concurrent-threads
  test that spawns a reconcile-protected SELECT and an
  ALTER at the same barrier, asserting the SELECT result
  is either pre-ALTER or SchemaStale — never mixed.
