# ADR 0022 — Verified offline migration from legacy SQL stores to shared-WAL domains

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** Implementation and regression tests committed to `main`; manual Cargo tests, crash fault injection and operational qualification required.  
**Depends on:** [ADR 0009 — typed rows](0009-typed-table-rows.md), [ADR 0015 — verified backup](0015-consistent-backup-restore.md), [ADR 0016 — durable catalog](0016-persistent-ddl-catalog.md), [ADR 0020 — shared-WAL domain](0020-shared-wal-multi-table-atomicity.md), [ADR 0021 — SQL shared-domain integration](0021-shared-domain-sql-integration.md).

## Problem

G12–G14 SQL stores placed each table's data under `tables/<TableId>/`, with an independent `rows.wal`, BRPG snapshot and exclusive file lock per table. G15–G16 instead require **one** shared-domain WAL and a registered TableId/schema for all tables participating in cross-table atomic DML. Pointing G16 at an existing G14 root without migrating row data would cause silent omissions or ambiguous write ownership.

G17 therefore implements a conservative **offline copy-and-verify cutover**. It does not modify or delete source row values or remove legacy directories; it publishes a **separate destination** only after validation. This is not online migration, replication, dual writing or a live zero-downtime cutover.

## Entry point

```rust
use bradian_relational::legacy_migration::migrate_legacy_sql;

let report = migrate_legacy_sql(
    "/srv/bradian/legacy",
    "/srv/bradian/shared-v1",
)?;
```

The destination must not exist, its parent must exist, and it cannot overlap the source. The source must have a valid durable `catalog/` directory. A previously unfinished staging directory is an error requiring explicit inspection, not an invitation to overwrite it.

**Precondition:** Stop or disconnect legacy applications and prevent any new writers before starting. Advisory locks only protect against cooperating writers.

## Migration protocol

1. Resolve source and destination paths and reject overlapping paths and an already existing destination or staging directory. Canonicalize the parent to make the target path explicit.
2. Open the source G11 `PersistentCatalog`, holding its native RowStore's exclusive lock until cutover publication. Clone its validated catalog definitions and record its acknowledged LSN.
3. Enumerate all entries under `source/tables/`. Reject noncanonical numeric table directory names, unknown table IDs, unexpected files and orphan stores. For each existing table, open its original `TableStore`, validate the schema pin and scan all typed values and physical keys. **Keep every source file lock open** until after verification and final publication. Catalog tables with no legacy directory are legitimate empty tables.
4. Create a sibling `.bradian-g17-incomplete` staging directory. Use the existing G10 catalog-only backup/restore to copy a recoverable, validated G11 journal into `staging/catalog/`. The G10 backup **can create a checkpoint in the source catalog**; it does not alter DDL event semantics or committed logical metadata. This is not a byte-for-byte read-only operation against the source.
5. Open a new `staging/shared-domain/` RowStore. Register every catalog table with the same stable `TableId` and pinned schema using the G15 validator. Reject unsupported secondary constraints and incompatible schema definitions.
6. Import source rows using G15 typed transactions in batches of at most 128 rows, below the existing 256-operation WAL limit. Each batch is one synchronized BRWL commit. Batches may be independently committed **inside the unpublished staging directory**; no partially populated destination is made visible to users.
7. Compare the destination's ordered typed rows for every table against the source scan. Publish a BRCP checkpoint of the imported shared domain.
8. Close and independently reopen the copied catalog and shared domain; require the identical catalog LSN and full table definitions, the expected table count and domain checkpoint LSN, and identical ordered rows. Re-read all original source tables and LSNs while their locks are still held, refusing any source inconsistency.
9. Synchronize the fully verified staging directory, rename it atomically to the absent destination path and synchronize its parent directory on Unix. Release source locks only when the function finishes.

The destination contains a durable `catalog/` and `shared-domain/` and **no legacy `tables/` directories**. It may therefore be opened by `DomainSqlEngine::open` and execute cross-table SQL BEGIN/COMMIT over its migrated records without applying any hidden conversion. The original legacy root remains available for independent verification and rollback of cutover planning, but subsequent writes to the new destination are not automatically mirrored back.

## Safety, failure and restart

- **Failure before final rename:** No destination is published. The deterministic staging directory is preserved for inspection; it is never automatically reused. A fresh attempt requires an operator-controlled quarantine/cleanup of the incomplete staging root after investigating the cause.
- **Failure during row import:** The incomplete staging RowStore can contain some durable WAL batches, but none are committed to the user-visible destination. Source rows and metadata remain available.
- **Existing destination:** Fail before touching it.
- **Source with active cooperating writer:** Fail when the existing catalog or any typed table exclusive file lock cannot be acquired.
- **Unknown legacy table directory:** Fail closed rather than ignore data.
- **Unprovisioned empty table:** Register an empty schema pin without inventing data.
- **Schema mismatch or invalid row:** Fail before publication.
- **Power loss:** Rename/sync behavior is a filesystem qualification requirement. Operators must retain the source until the destination has been reopened, verified and backed up.
- **Rollback:** G17 does not implement automatic reverse migration, failback of new shared-domain writes, or an atomic switch of application connection configuration. Those require a separately designed operational cutover manager.

## Important limitations

The source catalog and legacy table stores are independent WALs; before the migration locks every source file, no database-wide transaction boundary exists between them. **G17 requires legacy applications to be quiesced** and cannot guarantee a consistent capture in the presence of noncooperating writes. Lock acquisition is sequential and will fail if another cooperating process owns a relevant store, but it is not a substitute for a maintenance window.

This migration currently holds all imported rows in memory for full comparison and uses per-table transaction batches. Large datasets need bounded streaming, resume journals, checksummed import manifests, quotas and performance testing. A G10 catalog-only export may publish an additional source catalog checkpoint while preserving logical metadata.

The shared destination supports G16 multi-table DML atomicity; catalog DDL remains a separate WAL scope. Full catalog+data transactional DDL, secondary UNIQUE/FK enforcement, row-version MVCC, encryption, SQL network protocols and online failover are not added here.

## Evidence

`crates/bradian-relational/src/legacy_migration.rs` implements lock retention, source validation, staged copy/import, full re-open verification and exclusive publication. `crates/bradian-relational/tests/legacy_migration.rs` covers full migration and original-data preservation, new cross-table DML, active lock rejection, orphan table refusal, destination protection, repeated migration refusal, source/staging overlap, unfinished staging rejection and empty unprovisioned tables.

The tests and source are committed but **have not been executed in this environment**. Automatic CI remains disabled at the owner's request. Release acceptance requires manual Cargo check/test/Clippy, large database testing, process termination and power-loss fault injection, and a documented rollback procedure.
