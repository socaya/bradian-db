# ADR 0019 — Scoped SQL transaction sessions over one native table

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** Source and regression tests committed to `main`; independent Cargo compilation and cross-platform qualification outstanding.  
**Dependencies:** [ADR 0017 — native SQL execution](0017-native-sql-execution.md), [ADR 0018 — one-table transactions](0018-single-table-transactions.md).

## Context

G13 introduced transactional typed Rust operations and an application callback that directly stages typed mutations on one table. It did not parse SQL transaction-control statements. Users of the G12 SQL executor still needed a SQL-oriented way to group ordinary INSERT/SELECT/UPDATE/DELETE statements, observe their staged writes, and either commit a single durable WAL batch or discard the entire group.

A full database-wide transaction protocol is not currently available: catalog and individual table stores each have their own WAL and filesystem owner lock. Accepting arbitrary multi-table BEGIN/COMMIT would falsely imply atomic recovery across those files.

## Implemented decision

Introduce `SqlEngine::with_sql_session("database.schema.table", callback)`. The engine resolves **exactly one** durable catalog table, opens its typed store, holds its existing operation mutex and passes a scoped `SqlSession` to the caller. The session is bound to that table's lifetime; it cannot escape the callback while holding borrowed table storage.

`SqlSession::execute` accepts:

| SQL | Behaviour |
|---|---|
| `BEGIN` or `BEGIN TRANSACTION` | Capture one consistent row image and LSN via the G13 `TableStore::begin_transaction` |
| `SELECT ... FROM db.schema.table [WHERE full_pk]` | Return the transaction's stable snapshot merged with staged row changes |
| `INSERT INTO db.schema.table (...) VALUES (...)` | Validate typed values and stage a row |
| `UPDATE db.schema.table SET ... WHERE full_pk` | Read the session-visible row, validate non-key replacements and stage a change |
| `DELETE FROM db.schema.table WHERE full_pk` | Stage a deletion tombstone if the row exists |
| `COMMIT` | Consume the pending transaction and commit one bounded synchronized BRWL batch |
| `ROLLBACK` | Discard pending changes without writing WAL |

As in G12, WHERE equality must cover all primary-key fields. SQL statements must be fully parsed before execution. Unsupported SQL constructs—including DDL inside transactions, nested transactions, multiple statements in one string, and any reference to another table—are rejected.

A transaction is required before session DML. COMMIT and ROLLBACK without an active transaction are errors. A session whose callback returns with an open transaction drops its uncommitted staged mutations. On **any SQL statement error**, the session aborts its current transaction, ensuring an earlier valid statement is not accidentally committed after a later failed statement.

The existing typed-table transaction applies the same schema and primary-key validators as standalone SQL. A successful COMMIT writes and syncs one WAL frame; changes are visible only after that record is durable. Conflict rejection uses the table-global start LSN, so even a concurrent unrelated-key write may cause conflict. `SqlSession::last_commit_lsn()` exposes the most recent successful commit LSN within that scoped session; `in_transaction()` reports whether a transaction remains open.

## Scope and limitations

- **Atomicity:** All final modified keys on **one table's RowStore**, bounded by the existing batch limits (256 keys and 1 MiB serialized WAL body).
- **Consistency:** Enforced primary-key and typed-column rules for this profile. Secondary UNIQUE/FK/CHECK enforcement is not claimed.
- **Isolation:** A stable in-memory starting row snapshot and read-your-writes. Optimistic commit rejection protects against any intervening write to that table's WAL. It is not database-wide MVCC or independently configured SQL isolation.
- **Durability:** One acknowledged transaction is a synchronized BRWL batch, recovered or omitted as one complete frame.
- **No cross-table atomicity:** A session is explicitly tied to its one selected table. Attempting a different table returns an unsupported-feature error and aborts the staged transaction.
- **Not a wire-protocol session:** `with_sql_session` is a synchronous native Rust scoped callback. There is no external server-side connection, session ID, network authentication, or transaction state persisted across callback invocations.
- **No arbitrary SQL:** Joins, general predicates, updates affecting more than one PK, savepoints, `SELECT FOR UPDATE`, DDL inside the session, role policy and cross-table commit remain separate engineering work.

## Code and tests

Implementation: `crates/bradian-relational/src/sql.rs`; transactional storage: `typed_table.rs` and `row_store.rs`.

Five new integration tests in `tests/sql_execution.rs` cover SQL BEGIN/COMMIT with multiple row changes surviving reopen, explicit and implicit ROLLBACK, statement errors aborting all staged work, nested BEGIN and cross-table rejection, and COMMIT without an active transaction.

Source has been committed to `main`. **No Cargo tests or Clippy have been executed in this turn.** Automatic CI remains disabled by owner instruction. Manual acceptance must include workspace compile, crate and workspace tests, Clippy and filesystem fault injection at WAL sync.
