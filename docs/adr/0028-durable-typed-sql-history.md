# ADR 0028 — Durable typed SQL history and explicit shared-domain cutover

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** G23 code and regression tests committed to `main`; compilation, tests and fault-injection qualification pending.  
**Dependencies:** [ADR 0016](0016-persistent-ddl-catalog.md), [ADR 0020](0020-shared-wal-multi-table-atomicity.md), [ADR 0026](0026-typed-sql-mvcc-integration.md), [ADR 0027](0027-durable-mvcc-history.md).

## Problem

G21's typed SQL engine uses G20 in-memory version chains on the G15 `shared-domain/` layout; earlier versions cannot be read after a restart. G22 keeps multiple versions in the **different** `BRADIAN-DURABLE-MVCC-V1` physical format, keyed by user key and committed WAL LSN. Automatically opening an existing G15 RowStore as a G22 history store would silently reinterpret durable state or create a conflicting source of truth.

G23 provides an explicit **separate SQL history engine and verified offline migration**. It does not make G22 the default engine, change the G15 physical format, or claim that preexisting G16 historical versions can be resurrected: the source stores only its latest committed row image.

## Storage and schema contract

The G23 target root contains:

```text
<history-root>/
  catalog/               # original recovered G11 catalog, copied by verified G10 backup
  history-domain/        # independent G22 version-key RowStore with one authoritative WAL
```

Table schema pins in `history-domain/` use the established G15 logical key `\0bradian-domain-table-v1:` plus big-endian TableId, but they are stored as **G22 historical user-key versions**, never as plain RowStore values. The data keys retain the existing G15 `BRPK` table-id/composite-primary-key encoding and the typed `BRRW` row payload. The G22 `DVH1` key wrapper records the commit LSN of each change to either schema or row. A schema is currently immutable for this profile. Durable historical SQL reads verify that the matching schema pin **already existed at the requested LSN**, preventing a historical read from applying a present-day table definition to an earlier nonexistent table.

The adapter `DurableTypedStore` opens the independent history-domain, validates every persisted schema pin against the authoritative G11 catalog, rejects orphan physical rows and unexpected key namespaces, decodes every current row through its pinned typed definition and checks that the recomputed primary key matches the stored key. It never creates missing catalog definitions or pins during normal SQL startup. Row values are still constrained by G22's more restrictive inline capacity: 498-byte maximum user key and 3545-byte maximum user value before the G22 presence tag.

## SQL engine

`HistorySqlEngine` uses the G11 durable catalog and G22 `DurableTypedStore` with the exact G16 bounded SQL parser, typed literal binder and complete-primary-key predicate. Its synchronous `with_session` API supports BEGIN, COMMIT, ROLLBACK and typed single-row INSERT/UPDATE/DELETE/SELECT across several registered tables in one G22 WAL domain, with overlapping in-process sessions and per-key optimistic validation.

G23 adds read-only historical SQL syntax in scoped sessions:

```sql
BEGIN;
SELECT balance FROM bank.public.accounts AS OF 17 WHERE id = 101;
SELECT id, balance FROM bank.public.accounts AS OF 20;
ROLLBACK;
```

`AS OF` accepts an exact **unsigned, integer WAL LSN**. It is available only for SELECT immediately after a fully qualified `FROM` table and before an optional complete-primary-key `WHERE`; joins, arbitrary predicates, `AS OF` DML, multi-statement input, schema changes, SQL timestamps and concurrent DDL are not supported. Historical SELECT reads the retained committed version as of that requested LSN, not the session's staged writes. Nonhistorical SELECT maintains transaction start-snapshot visibility plus read-your-writes. A historical query before that table's durable schema registration, before G22's retained history floor, or beyond the latest committed WAL position is an error. A failed statement aborts the active session transaction.

The G22 native WAL persists each changed typed row/tombstone under its commit LSN as part of **one synchronized BRWL batch**. Thus a historical cross-table SQL commit is durable and all rows of one transaction become visible at the same commit LSN. G22's explicit retention floor can reclaim old versions; historical queries below that floor must fail, not return incomplete results.

## Offline migration

`history_migration::migrate_shared_to_history(source_root,destination_root)` is a separate **offline copy and verification** procedure:

1. Require a closed G16 `catalog/` and `shared-domain/` database with no nonempty legacy `tables/`, no preexisting `history-domain/` and **no G18 `lifecycle/` directory**. G18-managed databases require a future backup-aware cutover that copies and reconciles lifecycle metadata; this implementation fails closed rather than discard it.
2. Resolve paths and reject source/destination overlaps, existing destination and any prior `.bradian-g23-incomplete` staging directory.
3. Acquire and retain the exclusive G11 catalog and G15 domain RowStore locks until final cutover publication. Check every catalog table has an exactly matching registered source schema pin; refuse unknown/missing tables.
4. Capture all source table rows under a consistent shared-domain WAL snapshot, and record source catalog/domain LSNs. The migration is intended for a maintenance window with all cooperating writers stopped.
5. Copy the G11 catalog using G10 verified backup/restore into a sibling staging directory. That operation can create a source catalog checkpoint, but must not change acknowledged logical metadata.
6. Create a separate G22 history-domain WAL. Register the source schema pins as durable version-key values, then insert validated typed rows in bounded atomic 96-row WAL batches. A row too large for G22's stricter key/value capacity is an error, **never a truncation**.
7. Publish a G9 checkpoint, close the destination history domain and independently reopen the copied catalog plus the G23 typed history adapter. Compare original source and destination table definitions, table counts, exact typed row contents/order and their checkpoint/WAL positions.
8. Revalidate source catalog/domain LSNs while still holding source locks. Synchronize staging, refuse a destination that appeared during work, rename the verified staged root to the absent target and synchronize the parent directory on Unix.

A failed migration leaves the original catalog and shared-domain rows intact. The unpublished staging directory remains for operator inspection and is not silently retried. Ordinary `fs::rename` is atomic on qualified local filesystems but does not provide a portable race-free no-replace guarantee against a noncooperating destination creator.

**Historical semantics start at the destination import commits.** Source G16 transaction LSNs are *not* preserved as G22 historical commit positions. The first migrated version of a row is the acknowledged G23 import batch LSN; older source histories cannot be reconstructed from a latest-only physical store.

## Fail-closed compatibility

`HistorySqlEngine` refuses G16/G21 `shared-domain/` or G18 lifecycle roots, and does not create a missing history domain on open. The older `SqlEngine`, `DomainSqlEngine`, `MvccSqlEngine`, and `CatalogLifecycle` also refuse roots containing `history-domain/`, preventing accidental creation of a second, empty independently writable SQL storage domain next to G23's authoritative history.

No existing G16 or G21 database is modified in place by G23. Existing DAP analytical crates and `.derc` formats remain outside its scope.

## Remaining limitations and qualification

This slice is **not** a production SQL history server: it lacks schema-version migrations, G18-managed source conversion, G19 coordinated database-wide backup integration, SQL connection protocol, general joins/filters, serializable predicates, access control, encryption and cluster coordination. G22 history remains logically durable but the current engine loads retained historical key/version metadata in memory; no on-disk version-index query planner, bounds on historical retention or workload benchmarks are claimed.

G23 has twelve committed integration tests for explicit migration and original-source preservation, historical point and full-table SQL reads through restart/checkpoint, atomic cross-table updates, tombstone history, rollback, lock exclusion, ambiguous storage refusal and source/staging cutover protection.

**Build/test execution:** not performed in this environment. Manual Cargo check/test/Clippy, controlled fault-injection, source-data checksums, large migration performance, cross-platform filesystem sync/rename and operator rollback validation remain release prerequisites. Automatic GitHub CI stays manual-only.
