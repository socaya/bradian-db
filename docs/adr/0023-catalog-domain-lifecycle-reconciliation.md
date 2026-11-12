# ADR 0023 — Recoverable catalog-to-domain table provisioning

**Bradian Database Systems · Master Architecture and SRS v1.1**  
**Status:** Source implementation and regression tests committed to `main`; manual compilation and power-loss qualification pending.  
**Dependencies:** [ADR 0016 — persistent catalog](0016-persistent-ddl-catalog.md), [ADR 0020 — shared WAL](0020-shared-wal-multi-table-atomicity.md), [ADR 0021 — SQL frontend](0021-shared-domain-sql-integration.md), [ADR 0022 — offline migration](0022-verified-legacy-sql-migration.md).

## Problem

The durable G11 relational catalog and the G15 shared-WAL domain have **separate** authoritative journals. Creating a table definition in the catalog does not synchronously register a corresponding schema pin in the domain. Without lifecycle coordination, a process may crash between these steps, leaving a catalog table invisible to SQL or a domain pin that no longer agrees with catalog metadata.

The individual stores already guarantee locally synchronized WAL updates, but there is no distributed or database-wide two-phase commit. G18 therefore introduces a **recoverable, idempotent registration protocol** rather than pretending that two independent journals participate in a single ACID transaction.

## Decision and layout

`bradian-relational::lifecycle::CatalogLifecycle` opens the existing root's `catalog/` and `shared-domain/` with their current exclusive RowStore locks, plus a third native RowStore at `lifecycle/`. It owns all three until dropped. It refuses roots containing nonempty legacy `tables/`, because G17 migration must happen first.

```text
<bradian-root>/
  catalog/            # G11 durable DDL event journal
  shared-domain/      # G15 multi-table data + pinned schemas
  lifecycle/          # G18 durable table provisioning state machine
```

The lifecycle journal contains the marker `bradian.lifecycle.format = BRADIAN-TABLE-LIFECYCLE-V1` and one versioned JSON record under each binary key `table/<16-hex-digit-TableId>`. Each record contains an exact `Table` schema object and a `Pending` or `Active` state. The table is verified against the authoritative G11 catalog on every startup and reconciliation.

This is a recovery journal, not a replacement for catalog/table metadata or a global transaction log.

## Registration protocol

`provision_existing(TableId)` requires a table that already exists in the committed catalog. It cannot itself create or alter the catalog entry. For a new table:

1. Hold exclusive catalog, domain and lifecycle file locks.
2. Resolve the stable `TableId` and exact `Table` metadata from the durable G11 snapshot; reject unknown identifiers.
3. Write and synchronize `Pending` intent to the lifecycle RowStore.
4. Register the exact table schema in the G15 domain using its native idempotent schema-pin operation. This independently synchronizes a durable domain WAL entry.
5. Write and synchronize `Active` state to the lifecycle RowStore.
6. Return the domain commit LSN. The external SQL engine may access the table after this coordinator releases its locks and G16 opens the registered domain.

A repeated request for the same active table is idempotent if the exact matching domain pin exists. A changed or missing active pin is an integrity failure, not an opportunity to replace stored metadata.

## Crash/restart protocol

`CatalogLifecycle::open` calls `reconcile()` before returning:

- Verify the lifecycle journal marker, reserved key format, record version, `TableId`, and exact schema identity.
- Enumerate every durable G15 domain pin. Refuse pins that have no corresponding G11 catalog table or whose schema differs.
- Refuse lifecycle records for catalog identities that are absent or whose table metadata differs.
- Adopt existing **G16 pre-lifecycle pins** only when they exactly match durable catalog definitions. Record an `Active` lifecycle state without touching user row data.
- Replay every `Pending` entry by idempotently registering the exact schema pin, then writing an `Active` record.
- Reject an `Active` entry with a missing physical domain pin; do not silently recreate a previously active record and conceal data loss.

Because `Pending` precedes the domain pin and `Active` follows it, recovery is safe whether the process stopped after step 3, after step 4, or after step 5. The domain's schema pin is idempotent. Incomplete operations cannot be mistaken for an already active table when the physical pin is absent.

The recovery procedure can append lifecycle and domain WAL records during startup to complete pending work. Failures are propagated instead of reporting a ready database.

## Correctness boundaries

G18 provides **durable provisioning intent and restart reconciliation**, not atomic cross-store DDL transactions. It does not support:

- Creating a G11 catalog table and a domain pin as one uninterruptible commit.
- Transactional DROP, ALTER TABLE, schema migration, or reclaiming data/index pages.
- Multiple independent processes modifying catalog schema while the coordinator owns the root.
- MVCC, version-pinned SQL execution during concurrent DDL, distributed DDL or consensus.
- Database-wide consistent backup of the catalog, domain and lifecycle journal.

Operators must use a single lifecycle owner for provisioning and avoid independent catalog mutations that bypass this protocol. After provisioning, release the coordinator so `DomainSqlEngine` can acquire its usual exclusive catalog/domain locks.

A missing active domain pin remains a **hard integrity error**; automatic restoration requires verified backup/restore and an auditable repair workflow, not a blind re-registration. A valid `Pending` record with a matching catalog is automatically retried.

## Tests, release checks and traceability

`crates/bradian-relational/tests/catalog_lifecycle.rs` exercises end-to-end G18 provisioning followed by G16 cross-table SQL and restart; replay of pending intent before/after the domain pin; adoption of G16 pin metadata; refusal of unknown table IDs, orphan physical pins, damaged markers, unknown journal keys, missing active pins and legacy storage; and exclusive owner locking.

**Status:** These integration tests are committed but not yet executed. Manual `cargo check --workspace --locked`, `cargo test -p bradian-relational --locked`, `cargo test --workspace --locked`, and Clippy remain required, together with crash injection at each of the three synchronized WAL writes.

Automatic GitHub Actions remain disabled at the owner's request.
