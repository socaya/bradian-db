# ADR 0265 — Schema lease registry

## Status

Accepted.

## Context

ADR 0264 closed the race window inside a single
`execute_with_schema_snapshot`. But an operator planning an
online ALTER still had no way to count or wait for readers
that were about to run. P0-2 asks for a reader-drain
primitive the ALTER coordinator can poll.

## Decision

- `SqlEngine.schema_leases: Arc<Mutex<BTreeMap<TableId,
  BTreeMap<u64, usize>>>>` — per-table, per-version pin
  counts.
- `SchemaLease` RAII handle holds a `SchemaSnapshot` and a
  clone of the registry Arc. Dropping decrements the pin
  count for every `(TableId, version)` it observed; removes
  empty entries so introspection is clean.
- `SqlEngine::capture_schema_lease(names)` wraps
  `capture_schema_snapshot_of` and increments the pin
  counts.
- `SqlEngine::active_readers_on(table)` returns the sum of
  pins across all pinned versions for a table.
- `SqlEngine::reader_pin_versions(table)` returns the full
  `version → count` map for operator diagnostics.

## Consequences

- The lease is pure counting — ALTER does NOT currently
  block on non-zero readers. Integration with ALTER (wait
  for drain, reject on timeout) is a follow-up slice.
- Lease handles are not `Clone` — a lease is transferable
  only by move. Multiple concurrent readers each hold their
  own lease.
- `SchemaLease::snapshot()` exposes the captured
  `SchemaSnapshot` for direct use with
  `execute_with_schema_snapshot`.
- 7 cases in `tests/schema_lease.rs` cover: no leases = 0
  readers; single lease pins 1; drop releases; multiple at
  same version accumulate; different versions pin
  separately; lease carries snapshot; map entry drops when
  last lease goes.
