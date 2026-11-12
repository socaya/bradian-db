# ADR 0018 — Single-table optimistic transactions over Bradian WAL batches

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** Committed implementation and regression tests; not yet manually compiled or qualified.  
**Dependencies:** [ADR 0008](0008-native-row-wal-checkpoint.md), [ADR 0009](0009-typed-table-rows.md), [ADR 0012](0012-wal-backed-mutable-index.md), [ADR 0017](0017-native-sql-execution.md).

## Problem

Prior to G13, `TableStore` supported durable single-row INSERT/replace/DELETE and `RowStore` supported a single-frame atomic batch of up to 256 key mutations. Applications still had no public transactional table API to stage several dependent operations, read their own changes, cancel a group, or detect commits made since an initial read.

Creating arbitrary SQL `BEGIN` and `COMMIT` now would be misleading: the current catalog and individual table directories have independent WALs and cannot be committed as one atomic database-wide unit. A smaller, real transaction boundary is needed before implementing distributed or cross-table protocols.

## Decision

Introduce `TableStore::begin_transaction()`, returning a `TableTransaction` bound to one open typed table. The starting point is obtained through the new `RowStore::snapshot_with_lsn()` primitive, which retrieves the committed LSN and complete row image **under one mutex acquisition**. This prevents a torn snapshot in which the initial LSN and initial row set refer to different commits.

The transaction maintains a validated ordered snapshot and an in-memory ordered change map. `Some(row)` means a staged INSERT or replacement; `None` is a deletion tombstone. It exposes:

| Operation | Semantics |
|---|---|
| `start_lsn()` | Original committed sequence observed under the snapshot lock |
| `get(primary_key)` | Snapshot-consistent point read with read-your-writes |
| `scan()` | Stable ordered snapshot merged with staged changes |
| `insert(row)` | Validate schema, key and row capacity; refuse a visible duplicate |
| `replace(row)` | Validate full row; require a visible existing key |
| `delete(key)` | Require a visible key; stage a tombstone |
| `rollback()` | Discard all staged changes, without modifying WAL or stored rows |
| `commit()` | Convert final staged state to one atomic BRWL batch; reject intervening writes |

All inserts, replacements and deletes remain backed by the exact G4 row encoding and schema validation. The transaction never publishes a staged value to shared readers before `commit()`.

## Commit, atomicity and conflicts

`commit()` computes the final mutation for every modified key and calls `RowStore::commit_batch(Some(start_lsn), ...)`. The underlying RowStore performs key/value length checks and batch bounds checks before appending anything. It locks the store, compares the current committed LSN against the transaction start LSN, writes one CRC-protected frame containing all operations, synchronizes the WAL and only then updates the committed row map and incremental G7 index overlay.

An intervening mutation changes the store LSN and causes a **whole-transaction optimistic conflict**. No partial mutations are written. Because the check is against one global table-store LSN, even unrelated-key writes conflict. This is conservative, but avoids write skew and stale-read commits within a single store without introducing unqualified row-level locks.

A committed transaction consumes **one WAL sequence number** regardless of the number of distinct modified keys. It is constrained by the existing G3 batch limit of 256 operations and 1 MiB serialized WAL body. Failure due to batch size or value constraints must not append a partial WAL frame. If a physical write/sync fails, RowStore poisons itself until it is recovered. Incomplete final frames are not replayed after process termination.

A transaction with no changes is read-only: it returns its initial LSN without creating a WAL record. That result is a snapshot-read acknowledgment, not proof that the database is unchanged at return time.

## Isolation contract

The transaction supplies:

- A **stable initial row image** and read-your-writes for operations through that transaction object.
- Atomic commit and recoverability of all final staged rows **within one RowStore file/transaction frame**.
- Whole-store optimistic conflict rejection of writers that observed a stale initial LSN.
- Local in-process use while the owning TableStore retains its exclusive on-disk RowStore lock.

The transaction **does not** provide general MVCC, externally visible snapshot isolation across independent database tables, cross-table atomicity, SQL transaction sessions, global serializable SQL execution, predicate locks, distributed coordination, or SQL DDL transactionality.

The snapshot is an in-memory copy of all rows, so memory and latency are O(table size) at transaction start. The current G3 WAL frame size caps the number/size of staged changes. Production-scale MVCC and disk-resident snapshots require a separate implementation gate.

## SQL-engine entrypoint

`SqlEngine::transact_table("database.schema.table", closure)` resolves the catalog identity and opens one typed storage directory. Within the closure, applications can stage typed insert/replace/delete operations using the `TableTransaction` API. If the closure returns an error, dropping the transaction aborts all staged changes. If it succeeds, the engine calls `commit()` once and returns the caller's result together with the committed LSN.

This helper serializes work through the same SQL engine's operation mutex. It does not accept SQL `BEGIN`/`COMMIT` strings and does not combine catalog mutations or two different table stores in one transaction.

## Tests

`crates/bradian-relational/tests/table_transactions.rs` covers restart recovery of a multi-row commit under one LSN, rollback/drop without WAL append, atomic transfer updates, stale-transaction rejection, unrelated-key conflict behavior, snapshot reads across concurrent changes, delete-then-insert, rejected rows without partial publication, oversized batch rejection, concurrent worker contention and tombstones through checkpoint/restart.

`crates/bradian-relational/tests/sql_execution.rs` includes application-facing transaction helper tests for durable multiple-row writes, rollback on callback error and table resolution.

**Verification:** Source tests are committed but have **not** been executed in this turn. CI automation remains disabled by owner instruction. Qualification requires manual Cargo check/test/Clippy and crash-injection at the batch WAL sync boundary.
