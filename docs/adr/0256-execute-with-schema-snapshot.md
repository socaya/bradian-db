# ADR 0256 — `execute_with_schema_snapshot`

## Status

Accepted.

## Context

ADR 0242 defined `SchemaSnapshot` and its `reconcile`, but no
SQL caller used it. The primitive was dead until something
actually dispatched against it. This slice wires it into
`SqlEngine::execute` so callers can detect mid-plan schema
drift between autocommit calls.

## Decision

- New `SqlError::SchemaStale { table, captured, current }`.
- New `SqlEngine::capture_schema_snapshot_of(&[qualified_name])`
  — resolves each name against the live catalog (unknown
  names return `SqlError::Resolution`) and returns a
  `SchemaSnapshot` over just those tables.
- New `SqlEngine::execute_with_schema_snapshot(sql, &snap)` —
  reconciles the snapshot against the live catalog before
  dispatching through `execute`. Reconcile failure surfaces as
  `SqlError::SchemaStale` with the first observed divergence.
- New `SqlEngine::catalog_snapshot()` helper exposes the
  catalog for diagnostics.

## Consequences

- The primitive fires on autocommit boundaries, not inside a
  single `with_sql_session` call (sessions still hold the
  operation lock, so no concurrent ALTER is possible during
  them). The right client use is "capture once, run many
  statements over time, re-plan on `SchemaStale`".
- Scope limit: reconcile checks the versions at dispatch
  time. A raced ALTER that lands between reconcile and the
  inner `execute` is not detected by this slice — the
  operation lock inside `execute` makes that window short,
  but closing it fully requires taking the catalog mutex
  across both checks (future slice).
- 9 cases in `tests/schema_snapshot_execute.rs` cover: clean
  reconcile, detects rename-table / rename-column /
  drop-column / SET NOT NULL, multi-table snapshot only
  flags altered tables, unknown-table at capture rejects,
  control case (no snapshot = no stale check), error
  carries the specific table id.
