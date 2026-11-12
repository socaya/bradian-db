<p align="left">
  <img src="media/Bradian_Database_Systems_Primary_Horizontal_Blue.svg" width="330" alt="Bradian Database Systems" />
</p>

# Bradian Database Systems — Technical Documentation

**Owner:** Vitalworks Limited  
**Canonical source:** [socaya/bradian-db](https://github.com/socaya/bradian-db)  
**Primary architecture baseline:** Bradian Master Architecture & Software Requirements Specification **v1.1**

This directory contains the engineering specifications, architectural decisions, security design, and implementation evidence for Bradian Database Systems, the planned proprietary OLTP/OLAP/HTAP database. The architecture is **a target design**, not a statement that all its capabilities have been implemented.

## Read first

1. **[Master Architecture & SRS v1.1](Bradian_Master_Architecture_SRS_v1.1.md)** — normative component architecture, relational SQL, ACID/MVCC design, columnar analytics, HTAP consistency, cluster fabric, horizontal scaling, federation, security, disaster recovery, Bradian Studio, implementation guidance and acceptance gates.
2. **[Implementation audit and phased roadmap](implementation/BRADIAN_V1_1_AUDIT.md)** — maps existing source code to the SRS and records the not-yet-implemented engineering work.
3. **[Bradian security bootstrap guide](security/BOOTSTRAP_API_AUTH.md)** — local development authentication boundary and its important security limitations.
4. **[Relational catalog ADR](adr/0006-generic-relational-catalog.md)** — logical database/schema/table IDs, scope, constraints and migration limitations.
5. **[Portable page format ADR](adr/0007-portable-physical-page.md)** — versioned 4 KiB page format and corruption checks.
6. **[Native row WAL and checkpoint ADR](adr/0008-native-row-wal-checkpoint.md)** — synced row mutations, atomic batch commits, page snapshots and recovery.
7. **[Typed table ADR](adr/0009-typed-table-rows.md)** — persisted schema pinning, typed row format and primary-key integrity.
8. **[Disk B+tree index ADR](adr/0010-checkpoint-btree.md)** — immutable on-disk multi-level index, checkpoint integration and recovery boundaries.
9. **[Read-only buffer pool ADR](adr/0011-generation-scoped-buffer-pool.md)** — bounded LRU page caching and immutable generation safety.
10. **[Mutable index overlay ADR](adr/0012-wal-backed-mutable-index.md)** — WAL-committed logical delta and on-disk checkpoint B+tree merging.
11. **[Derived index recovery ADR](adr/0013-derived-index-reconstruction.md)** — rebuild absent checkpoint indexes from verified pages and audit physical index integrity.
12. **[Checkpoint publication ADR](adr/0014-atomic-checkpoint-manifest.md)** — checksummed generation commit, orphan quarantine and WAL-safe recovery.
13. **[Consistent backup/restore ADR](adr/0015-consistent-backup-restore.md)** — snapshot-plus-WAL capture, staged verification and safe restore.
14. **[Persistent relational catalog ADR](adr/0016-persistent-ddl-catalog.md)** — synchronized DDL events, stable object IDs, deterministic validated replay.
15. **[Native SQL execution ADR](adr/0017-native-sql-execution.md)** — parser, strict binder, native catalog/typed-row execution and supported SQL grammar.
16. **[Single-table transaction ADR](adr/0018-single-table-transactions.md)** — row snapshot, staged writes, rollback and atomic WAL batch commits.
17. **[Scoped SQL transaction ADR](adr/0019-scoped-sql-transaction-sessions.md)** — SQL BEGIN/COMMIT/ROLLBACK and staged DML for one table in a Rust callback.
18. **[Shared-WAL multi-table atomicity ADR](adr/0020-shared-wal-multi-table-atomicity.md)** — one WAL commit across registered typed tables in a dedicated transaction domain.
19. **[Shared-domain SQL integration ADR](adr/0021-shared-domain-sql-integration.md)** — explicit domain registration and SQL transaction sessions for multi-table atomic DML.
20. **[Legacy SQL migration ADR](adr/0022-verified-legacy-sql-migration.md)** — exclusive source locks, verified offline import and atomic new-destination cutover.
21. **[Catalog/domain lifecycle ADR](adr/0023-catalog-domain-lifecycle-reconciliation.md)** — WAL-backed Pending/Active table provisioning and restart reconciliation.
22. **[Database-wide backup ADR](adr/0024-database-wide-consistent-backup.md)** — consistent triple-WAL source locking, bound component receipts and staged verified restore.
23. **[MVCC foundation ADR](adr/0025-mvcc-row-version-foundation.md)** — pinned in-memory row versions, per-key first-committer-wins and WAL recovery of the latest state.
24. **[Typed MVCC SQL ADR](adr/0026-typed-sql-mvcc-integration.md)** — G11 catalog/G15 schema pin validation and concurrent scoped SQL sessions over G20 versions.
25. **[Durable MVCC history ADR](adr/0027-durable-mvcc-history.md)** — history keys persisted in one WAL batch, retained as-of reads and crash-safe version GC.
26. **[Durable typed SQL history ADR](adr/0028-durable-typed-sql-history.md)** — G22 version keys with G11/G15 typed metadata, SQL AS OF and verified G16-to-G23 cutover.

## Current milestone records

| Gate | Engineering scope | Current maturity |
|---|---|---|
| [BRD-G1](gates/BRD-G1-relational-catalog.md) | Relational catalog, typed column definitions, PK/FK metadata | In-memory schema management, no row data |
| [BRD-G2](gates/BRD-G2-page-format.md) | Versioned physical page representation and corruption detection | Page codec only; not yet durable storage |
| [BRD-G3](gates/BRD-G3-wal-row-store.md) | Synchronized key/value mutations, page checkpoints and WAL replay | Source and 17 tests committed; manual checks outstanding |
| [BRD-G4](gates/BRD-G4-typed-table-crud.md) | Typed table CRUD, schema identity, composite PKs, validation | Source/tests committed; manual checks outstanding |
| [BRD-G5](gates/BRD-G5-checkpoint-btree.md) | Immutable physical B+tree index with point and range lookups | Source/tests committed; manual checks outstanding |
| [BRD-G6](gates/BRD-G6-buffer-pool.md) | Bounded physical page cache, CRC validation, immutable-generation invalidation | Source/tests committed; manual checks outstanding |
| [BRD-G7](gates/BRD-G7-mutable-index-delta.md) | Incrementally updated WAL index overlay with tombstones and range merging | Source/tests committed; manual checks outstanding |
| [BRD-G8](gates/BRD-G8-index-recovery.md) | Crash-safe missing-index reconstruction and authoritative checkpoint integrity audit | Source/tests committed; manual qualification outstanding |
| [BRD-G9](gates/BRD-G9-checkpoint-publication.md) | Atomic committed checkpoint marker and recovery selection | Source/tests committed; manual checks outstanding |
| [BRD-G10](gates/BRD-G10-consistent-backup-restore.md) | Verified single-store snapshot/WAL backup and restore | Source/tests committed; manual acceptance outstanding |
| [BRD-G11](gates/BRD-G11-persistent-catalog.md) | Durable relational catalog and validated DDL journal | Source/tests committed; manual acceptance outstanding |
| [BRD-G12](gates/BRD-G12-native-sql.md) | Executable bounded SQL CREATE, INSERT, SELECT, UPDATE, DELETE | Source/tests committed; manual checks outstanding |
| [BRD-G13](gates/BRD-G13-single-table-transactions.md) | Atomic local single-table transactions, conservative conflict rejection and rollback | Source/tests committed; manual checks outstanding |
| [BRD-G14](gates/BRD-G14-scoped-sql-transactions.md) | Scoped single-table SQL BEGIN/COMMIT/ROLLBACK | Source/tests committed; manual checks outstanding |
| [BRD-G15](gates/BRD-G15-shared-wal-multi-table.md) | Shared-WAL atomic changes across multiple typed tables | Source/tests committed; manual checks outstanding |
| [BRD-G16](gates/BRD-G16-shared-domain-sql.md) | Explicit shared-WAL SQL BEGIN/COMMIT across registered tables; legacy storage blocked | Source/tests committed; manual checks outstanding |
| [BRD-G17](gates/BRD-G17-verified-legacy-migration.md) | Verified offline copy of legacy table data into shared-WAL SQL root | Source/tests committed; manual checks outstanding |
| [BRD-G18](gates/BRD-G18-catalog-domain-lifecycle.md) | Recoverable table provisioning, matching catalog/domain schema pins | Source/tests committed; manual checks outstanding |
| [BRD-G19](gates/BRD-G19-database-wide-backup.md) | Coherent local catalog/domain/lifecycle backup and verified new-root restore | Source/tests committed; manual checks outstanding |
| [BRD-G20](gates/BRD-G20-mvcc-foundation.md) | Opt-in WAL-backed row-level MVCC, pinned readers and version GC | Source/tests committed; manual checks outstanding |
| [BRD-G21](gates/BRD-G21-typed-sql-mvcc.md) | Opt-in typed SQL concurrent MVCC sessions with per-key write conflict detection | Source/tests committed; manual checks outstanding |
| [BRD-G22](gates/BRD-G22-durable-mvcc-history.md) | Independent durable row-version history with restart-safe as-of reads and manual GC | Source/tests committed; manual checks outstanding |
| [BRD-G23](gates/BRD-G23-durable-typed-sql-history.md) | Opt-in historical SQL and offline G16 shared-domain migration | Source/tests committed; manual checks outstanding |
| [BRD-G24](gates/BRD-G24-history-backup-restore.md) | Separate G23 history-domain archive with retained version and catalog verification | Source/tests committed; runtime qualification outstanding |
| [BRD-G25](gates/BRD-G25-lifecycle-history-migration.md) | Verified G18 lifecycle snapshot and offline copy into new G23 history root | Source/tests committed; runtime qualification outstanding |
| [BRD-G26](gates/BRD-G26-historical-schema-inspection.md) | Read-only immutable schema epochs and DESCRIBE TABLE AS OF | Source/tests committed; runtime qualification outstanding |
| [BRD-G27](gates/BRD-G27-additive-schema-compatibility.md) | Nullable-column schema compatibility planning and read-only historical projection | Source/tests committed; runtime qualification outstanding |
| [BRD-G28](gates/BRD-G28-durable-schema-intents.md) | WAL-backed inactive schema proposals and restart-time integrity validation | Source/tests committed; runtime qualification outstanding |
| Future gates | G18 lifecycle-aware history conversion, serializable SQL, atomic multi-store DDL, remote/PITR backup, versioned physical B+tree, HTAP, clusters and security | Must be implemented and qualified |

## Formal specification and assets

- [Master v1.1 — Markdown](Bradian_Master_Architecture_SRS_v1.1.md)
- [Master v1.1 — PDF](Bradian_Master_Architecture_SRS_v1.1.pdf)
- [Master v1.1 — editable DOCX](Bradian_Master_Architecture_SRS_v1.1.docx)
- [Master v1.0 — historical Bradian revision](Bradian_Database_Systems_Master_Architecture_SRS.md)
- [Approved blue horizontal logo](media/Bradian_Database_Systems_Primary_Horizontal_Blue.svg)
- [Approved white horizontal logo](media/Bradian_Database_Systems_Primary_Horizontal_White.svg)

**Diagram note:** The v1.1 Markdown references diagram image files that are not presently committed under `docs/diagrams/`. The PDF and DOCX are the retained figure-bearing master publications. Diagram-source assets should be restored in a later documentation maintenance change; do not imply the Markdown figure links are currently operational.

## Compatibility and historical material

Bradian originated from the DAP/DER Analytics Platform analytical prototype. The legacy documents below remain available **only as provenance and compatibility references**, not as the primary Bradian architecture:

- [Original DER Analytics Platform architecture](DER_Analytics_Platform_Architecture_and_Feature_Specification.md)
- [Original analytical requirements traceability](traceability.md)
- [Original analytical milestone records](gates/G1.md), [G2](gates/G2.md), and [G6](gates/G6.md)
- [Original DHIS2 analytics integration decision](adr/0005-dhis2-analytics-engine.md)

Legacy `dap-*` crate names, APIs, `.derc` storage and some DHIS2 integration identifiers are intentionally preserved to avoid breaking existing analytical data and clients. This does not change the product identity: **Bradian Database Systems**.

## Documentation policy

New engineering work should cite v1.1 requirement IDs and include design decisions and test evidence. Correctness, recovery, query isolation, privilege enforcement and cluster safety must be verified before a subsystem is described as completed. The source tree may contain interim implementations, and their limitations must remain explicit. Bradian's approved logo artwork must be reused without shape, spacing or lettering changes.
