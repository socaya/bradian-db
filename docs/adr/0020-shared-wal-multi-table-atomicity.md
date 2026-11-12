# ADR 0020 — Shared-WAL atomic transactions across multiple typed tables

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** Source and regression tests committed to `main`; compilation, tests and crash/power-loss qualification remain open.  
**Dependencies:** [ADR 0008 — WAL](0008-native-row-wal-checkpoint.md), [ADR 0009 — typed rows](0009-typed-table-rows.md), [ADR 0018 — optimistic table transactions](0018-single-table-transactions.md).

## Problem and rejected approach

G13 and G14 commit groups of writes to one table's `RowStore`, but every G4 table still has an independent WAL and checkpoint directory. Writing a `COMMIT` marker to an additional coordinator WAL without prepare, redo/replay ownership, durable participant decisions and idempotent recovery would not make two independent table WALs atomic. One table could be durable while another remained uncommitted after a crash.

Bradian's existing G3 storage already provides something smaller and proven in design: **one synchronized BRWL batch record can atomically carry 1–256 key/value mutations**, with a single monotonic LSN and replay that either accepts the entire frame or accepts none of it. This ADR applies that primitive across multiple typed table identities **only inside one shared RowStore**. It does not attempt to fake two-phase commit across independently owned G4 table files.

## Decision

Introduce `bradian-relational::transaction_domain::MultiTableStore`, a new storage layout with one `RowStore` owning the domain directory and one `BRWL` write-ahead log. Every table registered in that domain has a stable `TableId` and a serialized, versioned table schema pinned to a reserved row key:

```text
<domain>/
  rows.lock                  # exclusive single-process ownership
  rows.wal                   # authoritative ordered commit records
  checkpoint.current         # committed physical generation
  snapshot-<lsn>.brp         # CRC-checked physical row pages
  index-<lsn>.bri            # immutable physical B+tree over all domain keys
```

Table schema pin keys use the binary namespace `\0bradian-domain-table-v1:` plus the 8-byte big-endian `TableId`. User row keys are the existing `BRPK` typed, versioned composite primary keys and embed the 8-byte table identity. Because all data and schema rows live under the same RowStore, the physical B+tree, checkpoint manifest, backup and WAL recovery paths operate on one consistent generation.

The domain registers a table through `register_table(table)` using the **same `validate_table` implementation** as the G4 typed table store. Registration is a one-record synchronized WAL operation. Registering identical metadata is idempotent; attempting to change an already pinned table ID's schema fails. The current table model's secondary UNIQUE and FOREIGN KEY metadata are rejected at runtime until row-level enforcement exists. Registration itself is **not one atomic transaction with the separately persisted G11 catalog**.

## Transaction protocol

`MultiTableStore::begin_transaction()` clones the registered schema view and obtains one `RowStore::snapshot_with_lsn()` row image. Each persisted record is decoded against its pinned table definition, and physical keys are verified against the values' actual composite primary keys. Unknown table IDs, unrecognized key namespaces, wrong row types and mismatched keys are corruption errors.

The resulting `MultiTableTransaction` exposes:

| API | Meaning |
|---|---|
| `start_lsn()` | Exact shared-domain LSN at the beginning of the consistent snapshot |
| `get(table_id, primary_key)` | Reads that table's initial snapshot merged with its staged changes |
| `scan(table_id)` | Ordered rows from that table with read-your-writes |
| `insert(table_id, row)` | Validates typed values, key and inline row bounds, then stages insertion |
| `replace(table_id, row)` | Stages replacement only for a visible existing row |
| `delete(table_id, key)` | Stages a deletion tombstone only for a visible existing key |
| `rollback()` | Drops staged work without appending to WAL |
| `commit()` | Atomically writes every changed key across all registered tables |

Commit serializes the final operation for each distinct physical row key into one `RowStore::commit_batch(Some(start_lsn), mutations)` call. The RowStore validates all input before any WAL I/O. It compares the global LSN under its commit mutex, writes and synchronizes **one CRC-checked batch WAL frame**, then publishes every mutation to the committed map and the G7 index overlay under the same lock. A crash before the frame is fully accepted cannot expose a partial cross-table write set; replay of the complete frame restores the complete change set.

A write by any other caller to the domain—even in an unrelated table—advances the shared LSN and causes the whole tentative transaction to fail with `DomainError::Conflict`. This is intentionally conservative, global optimistic validation, not configurable SQL isolation or multi-version concurrency control.

The commit limit is the underlying G3 bound of **256 distinct row operations and 1 MiB WAL body**, together with the physical inline key/value capacity. A transaction with no mutations returns its starting LSN without adding a log frame.

## Durability, backup and restart

`MultiTableStore::open()` uses existing BRPG checkpoint recovery and BRWL replay. It recovers every pinned schema from the domain's authoritative store before interpreting any row. Every user row is then decoded with the correct table identity and verified primary-key encoding, refusing orphan or malformed records.

`checkpoint()` publishes the entire domain generation through the existing G9 BRCP manifest and associated B+tree. `backup_to()` captures and validates that entire RowStore using G10, including pinned schemas and all registered table rows. `restore_backup()` verifies backup source and restores only into a new directory; callers reopen the restored `MultiTableStore` to validate all table identities and rows. The **separate G11 relational catalog is not included** in this domain backup.

## Tests and residual risks

`crates/bradian-relational/tests/multi_table_transactions.rs` covers two-table one-LSN commit, restart recovery, explicit and implicit rollback, cross-table replacement/delete through checkpoint, stale optimistic conflicts, table-registration changes invalidating older transactions, immutable schema identities across restart, rejection of unknown tables/invalid values, batch-size limits, multi-threaded atomicity, duplicate keys, read-only commits, backup/restore and detection of orphan/corrupted physical rows.

The implementation does **not** migrate the existing G14 SQL tables or their standalone RowStore directories. It is a new explicit shared transaction domain, not a complete database-wide SQL engine transaction manager. A future migration/compatibility gate must define one authoritative storage layout and safe data transfer before cross-table SQL `BEGIN`/`COMMIT` can use this domain.

Also outstanding are catalog-plus-data atomic DDL, secondary constraints, row-level locking, serializable/MVCC semantics, cross-node consensus, unlimited transaction sizes, online index mutation and on-disk row versions.

**Verification status:** Files and tests have been committed to `main`, but Cargo compilation, test execution, Clippy and system fault injection have **not been completed** here. Automatic CI remains disabled as requested.
