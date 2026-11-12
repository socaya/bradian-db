# ADR 0029 — Durable catalog revision decisions

**Status:** Accepted for catalog-only DDL; coordinated historical activation remains deferred.

G33 proposals gain explicit activation and cancellation events in the existing version-1
JSON catalog journal. Each decision carries the exact proposal LSN and table definition; replay
rejects a missing or different pending proposal. Activation revalidates additive compatibility
against the current table, publishes one synced WAL event, then changes the in-memory table.
Cancellation removes only the pending proposal. Original proposals and decisions remain in
the append-only journal. Repeated decisions fail without advancing the WAL.

The operation mutex protects validation, WAL sync, publication and the returned commit LSN.
Pending proposals fence foreign-key changes and table drops. Added column names must already
be canonical catalog identifiers, and their types must pass the catalog type validator.
Existing reader snapshots remain immutable. IDs are never allocated by revision decisions.

This API changes catalog metadata only. It must not be exposed as SQL ALTER against an
existing row domain: typed storage schema pins intentionally reject a changed catalog.
G30 history coordination fences continue to reject independently advanced catalog WALs.
A future cross-WAL recovery protocol must coordinate these decisions before live historical
schema evolution is enabled. No existing row bytes or schema pins are rewritten here.
