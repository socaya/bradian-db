# ADR 0266 + 0267 — SchemaSnapshot in DomainSqlEngine / MvccSqlEngine sessions

## Status

Accepted.

## Context

ADR 0256 wired `SchemaSnapshot` into `SqlEngine::execute`.
The shared-domain (`DomainSqlEngine`) and MVCC
(`MvccSqlEngine`) engines run multi-statement sessions and
had no snapshot integration — long-running sessions could
observe a schema version shift without any signal.

## Decision

Symmetric pattern on both engines:

- `DomainSqlSession.schema_snapshot: Option<SchemaSnapshot>`
  (new field) + `attach_schema_snapshot(snap)` setter.
- `DomainSqlSession::execute` reconciles the attached
  snapshot against the engine's live catalog at the top of
  every statement. Failure surfaces `SqlError::SchemaStale`,
  aborts any in-flight transaction, and clears `self.active`.
- `DomainSqlEngine::capture_schema_snapshot_of(names)` for
  capture.
- Same three additions on `MvccSqlEngine` / `MvccSqlSession`.
- New public `SchemaSnapshot::from_entries(entries)` for
  operator tooling and test scaffolding — lets callers
  construct a snapshot with specific `(TableId, version)`
  pairs without going through `Catalog::capture_...`.

## Consequences

- Sessions that opt in will fast-fail when the schema shifts
  under them (via the ADR 0266/0267 reconcile) instead of
  silently returning mixed-schema results.
- Sessions that do NOT attach a snapshot keep their old
  behavior — zero cost for callers who don't want the
  guarantee.
- The reconcile reads the engine's own catalog. The
  `DomainSqlEngine` and `MvccSqlEngine` each own their own
  `PersistentCatalog`; cross-engine coordination is already
  blocked at the MultiTableStore pin level, so the
  within-engine snapshot is the right coordination unit.
- Full on-disk coordination between SqlEngine mutations and
  DomainSqlEngine pins (so an external ALTER through
  SqlEngine propagates to DomainSqlEngine's pin) is a later
  slice.
- 4 cases in `tests/snapshot_session_engines.rs` cover:
  domain session detects stale snapshot; domain session
  without snapshot is unchanged; mvcc session detects
  stale snapshot; mvcc session without snapshot is
  unchanged. Both detect paths use the new
  `SchemaSnapshot::from_entries` to fabricate a
  deliberately-stale snapshot since cross-engine ALTER
  through the base `SqlEngine` breaks the MultiTableStore
  pin invariant (a separate gap).
