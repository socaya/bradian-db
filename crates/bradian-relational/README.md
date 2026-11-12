# Bradian Native Relational Storage

**Package:** `bradian-relational`  
**Target specification:** Bradian Database Systems Master Architecture & SRS v1.1

This crate contains three operational building blocks that remain separate from the legacy DAP analytical engine and DHIS2-specific schema semantics:

1. **Coordinated whole-database backup and restore.** `CatalogLifecycle::backup_database_to` locks catalog/domain/lifecycle WALs and publishes a manifest-bound, independently verified triple-component archive. `database_backup::restore_database` checks all component CRCs and restores only into a new root after full G18 reconciliation.
2. **Recoverable catalog/domain lifecycle coordination.** `CatalogLifecycle` journals Pending/Active intents, idempotently pins existing catalog tables in the shared WAL, resumes interrupted registration and rejects orphaned/missing pins during restart.
3. **Verified offline legacy SQL migration.** `migrate_legacy_sql` locks catalog and legacy table files, copies validated typed rows to a staged shared-WAL domain, verifies imported catalog/schema/rows and publishes a separate destination only after successful reopen; source rows remain available.
4. **Shared-domain SQL transactions.** `DomainSqlEngine` binds durable catalog TableIds to an explicitly registered shared-WAL domain and supports scoped SQL BEGIN/COMMIT/ROLLBACK with cross-table INSERT/SELECT/UPDATE/DELETE; refuses roots containing legacy table stores.
5. **Shared-WAL cross-table transaction domain.** `MultiTableStore` registers stable typed TableIds in one dedicated RowStore, and `MultiTableTransaction` commits changes across those tables in one WAL batch; includes snapshot reads, rollback, conflict validation and whole-domain recovery. Existing SQL table directories are separate and not automatically migrated.
4. **Scoped SQL transaction sessions.** `SqlEngine::with_sql_session` accepts SQL BEGIN, SELECT, INSERT, UPDATE, DELETE, COMMIT and ROLLBACK within one selected table. It uses G13 snapshots and WAL batches; it does not support cross-table SQL transactions.
5. **Single-table atomic transaction execution.** `TableStore::begin_transaction` provides stable snapshot reads, read-your-writes, rollback and one-frame WAL commit with conservative LSN conflict detection; `SqlEngine::transact_table` exposes this to Rust clients.
6. **Native SQL execution profile.** `SqlEngine` parses, validates, resolves and executes a strict CREATE/INSERT/SELECT/UPDATE/DELETE subset using native persistent catalog and typed table storage. Its DML uses full-primary-key equality or SELECT scans, not arbitrary SQL expressions.
3. **WAL-backed persistent relational catalog.** `PersistentCatalog` commits validated database/schema/table DDL and foreign-key definitions through synchronized journal events, preserves stable object IDs across restart, rejects corrupted or missing events, and supports `backup_catalog_to` for verified metadata-only backups. The original in-memory `Catalog` remains the validator.
4. **Portable physical pages.** The versioned `BRPG` 4096-byte binary page codec, explicit little-endian fields, CRC32 integrity verification, and strict decoding.
5. **Consistent backup and restore.** `RowStore::backup_to` and `backup_current_generation` capture an atomic manifest/snapshot/index/WAL state, verify the staged copy, and publish a backup directory. `RowStore::restore_backup` validates the copied files and restores only to a new directory.
6. **Atomic checkpoint publication.** A versioned, CRC-checked `BRCP` manifest selects the committed physical generation; writes preserve WAL until publication succeeds and quarantine interrupted orphan snapshots before retrying.
7. **Derived index recovery and audit.** A missing B+tree is reconstructed from validated BRPG checkpoint records on restart; `RowStore::verify_checkpoint_index()` audits every physical pointer and leaf ordering against the authoritative checkpoint. Damaged published indexes are never silently overwritten.
8. **WAL-backed mutable logical index.** Each committed mutation updates an ordered delta over the published disk B+tree; point reads and ranges merge unchanged checkpoint rows with inserted/replaced values and deletion tombstones, rebuilding the delta from WAL on restart.
9. **Generation-scoped physical buffer pool.** `PageCache` bounds resident verified 4 KiB pages, applies LRU eviction and rejects CRC/page-ID/LSN errors; the index and row snapshot caches are retained across queries until checkpoint generation changes.
10. **Disk-resident B+tree indexing.** A multi-level immutable physical index is built at each checkpoint and supports real on-disk point and ordered-range lookups over checkpoint rows. New WAL writes bypass the stale index until the next checkpoint.
11. **Typed persistent table CRUD.** `TableStore` validates the catalog schema on every open, stores a durable copy of the table definition, supports typed INSERT/replace/DELETE/get/scan, and rejects duplicate or invalid primary keys.
12. **Durable single-process row storage.** `RowStore` performs individually atomic insert/replace/delete operations and bounded multi-key WAL batch commits, syncs the `BRWL` before publishing each write, loads `.brp` checkpoint pages, replays committed log records on restart, repairs a truncated *final* WAL frame, and rejects corruption.

## Running the implementation

```rust
use bradian_relational::row_store::RowStore;

fn example() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::temp_dir().join("bradian-row-example");
    let store = RowStore::open(&root)?;
    store.put(b"customer:1", b"active")?;
    assert_eq!(store.get(b"customer:1").as_deref(), Some(&b"active"[..]));
    store.checkpoint()?;
    Ok(())
}
```

```sh
cargo test -p bradian-relational --locked
```

The Rust API provides `open`, `put`, `delete`, `commit_batch`, `get`, `scan`, `indexed_get`, `indexed_range`, `len`, `last_lsn`, and `checkpoint`. The disk B+tree accelerates only fully checkpointed versions and reads the corresponding verified physical row page. A `commit_batch` accepts 1–256 operations in a single bounded WAL frame and an optional expected LSN for optimistic conflict validation; a successful batch receives one durable LSN. It accepts binary keys of 1–512 bytes and inline values up to 3,546 bytes. A `parking_lot` mutex serializes writers and reads within one process, and an OS-level exclusive advisory file lock prevents a second cooperating process from opening the same store. Rust 1.89+ is required. A `put` or `delete` return value is its synchronized WAL sequence number. A snapshot rebuilds all records onto 4 KiB physical pages and is published before WAL truncation.

**Important limitations:** This crate is **not** a SQL server, persistent catalog, general multi-statement transaction manager, MVCC engine, on-disk B+tree, multi-node lock manager, encrypted database, or fully tested cross-platform power-loss implementation. A second RowStore open for an already-locked directory is rejected. External programs must not bypass the advisory lock. Full transactions, isolation, deferred constraint enforcement, cross-platform lock qualification, key management, page allocation/free lists and filesystem failure injection remain separate mandatory engineering gates.

See [ADR 0024](../../docs/adr/0024-database-wide-consistent-backup.md), [G19](../../docs/gates/BRD-G19-database-wide-backup.md), [ADR 0023](../../docs/adr/0023-catalog-domain-lifecycle-reconciliation.md), [G18](../../docs/gates/BRD-G18-catalog-domain-lifecycle.md), [ADR 0022](../../docs/adr/0022-verified-legacy-sql-migration.md), [G17](../../docs/gates/BRD-G17-verified-legacy-migration.md), [ADR 0021](../../docs/adr/0021-shared-domain-sql-integration.md), [G16](../../docs/gates/BRD-G16-shared-domain-sql.md), [ADR 0020](../../docs/adr/0020-shared-wal-multi-table-atomicity.md), [G15](../../docs/gates/BRD-G15-shared-wal-multi-table.md), [ADR 0019](../../docs/adr/0019-scoped-sql-transaction-sessions.md), [G14](../../docs/gates/BRD-G14-scoped-sql-transactions.md), [ADR 0018](../../docs/adr/0018-single-table-transactions.md), [G13](../../docs/gates/BRD-G13-single-table-transactions.md), [ADR 0017](../../docs/adr/0017-native-sql-execution.md), [G12](../../docs/gates/BRD-G12-native-sql.md), [ADR 0016](../../docs/adr/0016-persistent-ddl-catalog.md), [G11](../../docs/gates/BRD-G11-persistent-catalog.md), [ADR 0015](../../docs/adr/0015-consistent-backup-restore.md), [G10](../../docs/gates/BRD-G10-consistent-backup-restore.md), [ADR 0014](../../docs/adr/0014-atomic-checkpoint-manifest.md), [G9](../../docs/gates/BRD-G9-checkpoint-publication.md), [ADR 0013](../../docs/adr/0013-derived-index-reconstruction.md), [G8](../../docs/gates/BRD-G8-index-recovery.md), [ADR 0012](../../docs/adr/0012-wal-backed-mutable-index.md), [G7](../../docs/gates/BRD-G7-mutable-index-delta.md), [ADR 0011](../../docs/adr/0011-generation-scoped-buffer-pool.md), [G6](../../docs/gates/BRD-G6-buffer-pool.md), [ADR 0010](../../docs/adr/0010-checkpoint-btree.md), [G5](../../docs/gates/BRD-G5-checkpoint-btree.md), [ADR 0009](../../docs/adr/0009-typed-table-rows.md), [G4](../../docs/gates/BRD-G4-typed-table-crud.md) and [ADR 0008](../../docs/adr/0008-native-row-wal-checkpoint.md), [ADR 0007](../../docs/adr/0007-portable-physical-page.md) and the [v1.1 specification](../../docs/Bradian_Master_Architecture_SRS_v1.1.md). Rust tests are included, but automatic CI is disabled under the present repository policy.
