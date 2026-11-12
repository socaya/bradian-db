# BRD-G14 — Scoped SQL BEGIN/COMMIT/ROLLBACK on one table

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Design record:** [ADR 0019](../adr/0019-scoped-sql-transaction-sessions.md)  
**Implementation:** `crates/bradian-relational/src/sql.rs`, using G13 `TableTransaction`  
**Status:** Source and integration tests committed directly on `main`; manual verification outstanding.

## Implemented

- [x] `SqlEngine::with_sql_session` establishes one native table session scoped to a Rust callback.
- [x] SQL `BEGIN`/`BEGIN TRANSACTION` create a consistent G13 transaction snapshot.
- [x] Session `SELECT` reads its initial snapshot plus staged changes.
- [x] Session `INSERT`, `UPDATE` and `DELETE` stage typed validated changes.
- [x] Session `COMMIT` publishes all final changes through one synchronized WAL batch.
- [x] SQL `ROLLBACK`, callback exit and statement errors discard uncommitted changes.
- [x] Nested BEGIN, cross-table access, DDL and unsafe SQL are rejected.
- [x] `in_transaction` and `last_commit_lsn` expose local session status.
- [x] Five SQL session tests cover durable commit, restart, rollback, abort and unsupported operations.

Checkboxes denote implementation/tests **present in source**, not observed passing tests.

## Manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Further qualification requires error injection at WAL append/fsync, disk-full and restart boundaries. Automatic CI remains disabled at the owner's request.

## Still open

- [ ] Multi-table atomic WAL transaction coordinator and crash-safe recovery.
- [ ] SQL wire protocol, independent client sessions, authentication and resource cleanup.
- [ ] SQL-compatible isolation levels, MVCC, predicate locks, deadlock handling and savepoints.
- [ ] Transactional DDL coordinated with catalog and table data.
- [ ] Row-level foreign-key/secondary unique constraint enforcement.
- [ ] Persistent mutable physical B+tree with WAL-before-data page flushing.

**Do not classify the G14 scoped Rust callback as full multi-table SQL ACID or production SQL transaction-server support.**
