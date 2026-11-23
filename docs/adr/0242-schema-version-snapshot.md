# ADR 0242 — Per-table schema_version and SchemaSnapshot

## Status

Accepted.

## Context

Master-SRS P0-2 lists concurrent online DDL as a Gate 1
blocker. The base `Catalog` currently has no way for a reader
to detect that a schema shifted between resolution and
completion. Without that primitive, every online-DDL approach
(reader fencing, optimistic retry, schema leases) is stuck at
step zero.

## Decision

- `Catalog` carries `schema_versions: BTreeMap<TableId, u64>`,
  tagged `#[serde(default)]` so older serialized catalogs load
  cleanly with an empty map.
- Every schema-mutating catalog method calls
  `bump_schema_version(id)` after the mutation commits in-memory.
  CREATE sets the initial version to 1. DROP removes the
  entry. No-op mutations (same-type widen, drop-already-absent
  default, remove-already-absent policy) do not bump.
- `Catalog::schema_version(id)` returns `Some(u)` for live
  tables and `None` for unknown/dropped ids.
- `Catalog::capture_schema_snapshot(ids)` and
  `Catalog::capture_full_schema_snapshot()` return a
  `SchemaSnapshot` carrying each observed (table, version).
- `SchemaSnapshot::reconcile(&catalog)` returns
  `Err(SchemaStale { table, captured, current })` at the first
  observed table whose live version does not equal its
  captured version (a dropped table reports `current = 0`).

## Consequences

- Foundational primitive only. This ADR does NOT wire the
  snapshot through any SQL path or enforce fencing — readers
  must opt in by capturing and reconciling explicitly. Those
  integrations land in a follow-up slice.
- Versions are in-memory; they are rebuilt from the catalog
  journal on reopen. Persistence across restart would record
  identical shapes since replay invokes the same Catalog
  methods. No cross-process coordination.
- 14 cases in `tests/schema_version_snapshot.rs` cover: CREATE
  sets version=1, unknown-id returns None, rename-table /
  rename-column / drop-column / SET NOT NULL / widen bump,
  no-op widen does not bump, DROP removes, reconcile on
  unchanged catalog succeeds, reconcile after a bump fails,
  unobserved tables are ignored, dropped observed tables
  surface SchemaStale with current=0, full snapshot includes
  every table.
