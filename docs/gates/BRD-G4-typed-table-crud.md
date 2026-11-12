# BRD-G4 — Typed table CRUD, durable schema pinning and primary-key enforcement

**Source:** Bradian Database Systems Master Architecture & SRS v1.1  
**Decision record:** [ADR 0009](../adr/0009-typed-table-rows.md)  
**Implementation:** `crates/bradian-relational/src/typed_table.rs`  
**Gate status:** Source and tests committed on `main`; acceptance checks not executed automatically.

## Implemented capabilities

The generic catalog's `Table` metadata is now used by a fully functional, persisted typed-table API. Every table opened through `TableStore` is tied to its own `RowStore` directory, with a WAL-persisted schema definition to guard against accidental reuse or incompatible changes.

Operations are `insert`, `replace`, `delete`, `get`, `scan` and `checkpoint`. Insert checks duplicate primary keys and commits via the existing optimistic single-LSN WAL batch mechanism. Replace and delete require that the primary key already exists. The stored row format has explicit version, table ID, typed scalar tags and length bounds. Point reads and scans validate the stored row's primary key against its physical key.

Supported value types include Boolean, Int32, Int64, exact unscaled Decimal, bounded Varchar, Text, TimestampTz (UTC microseconds), UUID and NULL where allowed. Keys can comprise more than one typed component without concatenation collisions. Unsupported secondary UNIQUE and FOREIGN KEY runtime constraints cause table opening to fail rather than being silently skipped.

## Evidence in source tests

- [x] Reopen after INSERT, UPDATE (replace), DELETE and checkpoint.
- [x] Duplicate primary key rejection after restart.
- [x] Mismatched schema, table ID and column types rejected on reopen.
- [x] Nullability, numeric precision, string bounds and type checks.
- [x] Composite primary keys with unambiguous encodings.
- [x] Supported typed scalar roundtrip, invalid bytes and malformed rows.
- [x] Concurrent primary-key insertion: exactly one commit.
- [x] Invalid replace/delete preconditions.
- [x] Unsupported constraints rejected.

These checkboxes mean **the tests exist in source**, not that they were executed in this turn.

## Remaining requirements before production qualification

- [ ] Manual workspace compilation and full Rust tests.
- [ ] Filesystem fault injection and platform-specific power-loss testing.
- [ ] Persistent transactional catalog with versioned DDL.
- [ ] Secondary unique indexes, FK enforcement, CHECK constraints and cascade semantics.
- [ ] Multi-table atomic transactions and MVCC/SQL isolation.
- [ ] B+tree physical indexes, buffer pool and page reuse.
- [ ] SQL parser, binder, optimizer, execution and drivers.
- [ ] Storage/WAL encryption, key rotation and auditable authorization.

**Do not claim full OLTP, ACID/MVCC or SQL completion from this milestone.** Atomic local WAL batches and primary-key integrity are important building blocks, but they are not a substitute for a general transaction manager.
