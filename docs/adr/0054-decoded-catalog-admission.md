# ADR 0054 — Decoded persistent-catalog admission (G1-MEMORY)

**Status:** Accepted for `PersistentCatalog` decoded-state and journal-event
byte budgets. Gate 1 remains active.

The persistent catalog has an append-only DDL journal and an in-memory
`Catalog` + pending-revisions map rebuilt from that journal at open. Both
grew without admission: an operator or hostile source could commit DDL until
the decoded in-memory state exceeded available RAM, and a journal replay on
restart would re-materialise the same state without any budget check.

`CatalogLimits { max_databases, max_schemas, max_tables, max_revisions,
max_event_bytes }` is added to `runtime_limits.rs`. Defaults: 1024 databases,
4096 schemas, 65_536 tables, 1024 pending revisions, 64 KiB per DDL event.
These are per-catalog counts; they do not reserve total process memory.
`ResourceError::CatalogLimit { resource, limit }` is distinct from catalog
semantic errors, storage errors, JSON encoding errors and corruption.

`PersistentCatalog::open_with_limits(path, limits)` accepts a policy and
records it on the catalog instance (not persisted on disk). The default
`PersistentCatalog::open` retains its existing signature and uses
`CatalogLimits::default()`. `resource_limits()` and `resource_usage()`
expose the configured policy and current counts.

Enforced invariants:

1. During live DDL (`commit`), after the tentative catalog and revisions map
   are updated by the replay validator, `CatalogLimits::check` runs on the
   new counts and `check_event_bytes` runs on the serialized event. A
   refusal returns `ResourceError::CatalogLimit`, leaves the live catalog,
   revisions and journal exactly as they were, and never writes to the WAL.
2. During open replay, each event's stored bytes are admitted before decode,
   and the decoded counts are checked after each successful `event.apply`.
   A refusal returns the same resource error and never trims the journal or
   publishes a partial catalog.

Not enforced by this ADR and explicitly open:

- Per-table column, FK and unique-key counts (bounded instead by the
  RowStore inline-record limit on serialized DDL event bytes).
- Decoded catalog state across multiple domains or engines.
- Global process-wide decoded-state totals.
- A versioned catalog compactor; the journal is still append-only, so
  accepted DDL cannot be forgotten by simply dropping objects.

No persistent format, wire protocol or SQL grammar changes. `CatalogLimits`
is not written to disk. Existing callers see identical behavior under the
defaults; none of the shipped tests hit the default thresholds.
