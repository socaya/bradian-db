# ADR 0021 — Explicit SQL integration with a shared-WAL transaction domain

**Bradian Database Systems · Master Architecture & SRS v1.1**  
**Status:** Code and integration tests committed to `main`; manual compile, test and crash-injection qualification remain outstanding.  
**Depends on:** [ADR 0016](0016-persistent-ddl-catalog.md), [ADR 0017](0017-native-sql-execution.md), [ADR 0019](0019-scoped-sql-transaction-sessions.md), [ADR 0020](0020-shared-wal-multi-table-atomicity.md).

## Problem

G15 proved a multi-table atomic storage unit is possible if tables share **one** RowStore, commit LSN, WAL, snapshot and physical B+tree. However, G12–G14 SQL tables are stored in separate `tables/<TableId>/` directories and do not acquire cross-table atomicity by invoking an independent coordinator. A fully automatic switch would risk silently abandoning data and allowing ambiguous write ownership.

This decision adds a separate, explicit SQL frontend over the shared-WAL domain, without pretending that legacy row files have been migrated.

## Implementation

`bradian-relational::domain_sql::DomainSqlEngine` opens a durable catalog from `<root>/catalog/` and a G15 shared domain from `<root>/shared-domain/`. It reuses the same G12 lexer, parser, typed literal binding, column lookup and complete primary-key predicate rules, ensuring SQL values are validated consistently.

The engine refuses to open any root whose `<root>/tables/` directory contains legacy table entries. A clean G12 catalog with no provisioned legacy table storage can be reused after its old `SqlEngine` instance has closed. The rejection is deliberate and fail-closed; G16 **does not** implement a migration or schema lifecycle conversion.

All domain tables must be explicitly activated with `register_catalog_table("database.schema.table")`. The registration resolves the name in the durable G11 catalog and pins exactly that TableId/schema into the G15 shared WAL. Repeated identical registrations are idempotent and incompatible schema reuse fails. This method is separate from G11 DDL: it cannot make a catalog creation and a domain registration one atomic transaction.

## Transactional SQL

`DomainSqlEngine::with_session(callback)` provides a scoped `DomainSqlSession`. It accepts:

| SQL | Semantics |
|---|---|
| `BEGIN` / `BEGIN TRANSACTION` | Create one consistent snapshot and starting LSN over **all** registered shared-domain tables |
| `INSERT INTO database.schema.table (...) VALUES (...)` | Validate all named columns and typed literal values; stage a new row |
| `SELECT columns FROM database.schema.table` | Read initial snapshot plus staged changes for that table |
| `SELECT ... WHERE full_primary_key` | Use a complete primary-key equality predicate (including composite keys) |
| `UPDATE ... SET ... WHERE full_primary_key` | Stage typed replacements to one existing key |
| `DELETE ... WHERE full_primary_key` | Stage a deletion tombstone |
| `COMMIT` | Publish **all** staged changes across multiple tables in **one** BRWL batch and one LSN |
| `ROLLBACK` | Drop all staged changes without a WAL mutation |

A SQL statement failure aborts the active transaction; calling COMMIT afterward fails. Exiting a scoped session with an open transaction implicitly discards it. Nested BEGIN, DDL, multi-statement strings, unknown/unregistered tables and unsupported SQL operators fail. The session serializes use within the engine through a `parking_lot::Mutex`.

A global optimistic LSN check rejects a write transaction if any other shared-domain write or table registration occurred since the snapshot. The G15 `RowStore::commit_batch` enforces the existing maximum of 256 final changed keys and 1 MiB WAL frame body. A complete synchronized WAL frame is replayed as one unit, so one table cannot recover a committed transfer while another fails to recover its movement entry.

The SQL frontend returns existing `SqlResultSet::Rows` and `SqlResultSet::Command` values. Domain failures are propagated as structured `SqlError::Domain` rather than flattened to generic unsupported errors.

## Example integration

The metadata definitions may be created beforehand through the existing durable catalog or a **closed** G12 `SqlEngine` when it has not provisioned any legacy table directories.

```rust
let engine = DomainSqlEngine::open(root)?;
engine.register_catalog_table("banking.public.accounts")?;
engine.register_catalog_table("banking.public.movements")?;

engine.with_session(|session| {
    session.execute("BEGIN")?;
    session.execute("UPDATE banking.public.accounts SET balance=800 WHERE id=101")?;
    session.execute(
        "INSERT INTO banking.public.movements (id, account_id, amount) VALUES (99,101,-200)"
    )?;
    session.execute("COMMIT")?;
    Ok(())
})?;
```

This is a scoped native Rust callback API, not a remote SQL server connection or persistent wire-protocol session.

## Durability and remaining boundaries

- **Atomic row commit:** Works across registered tables inside one `shared-domain` WAL, not across multiple unrelated RowStores.
- **Catalog:** The G11 catalog continues to own independently WAL-backed metadata. Catalog DDL and row data are not a single atomic commit.
- **Legacy rows:** Existing G12–G14 `tables/<id>` directories are explicitly refused; G16 supplies no automatic migration or import. A separately designed, verified offline migration and cutover procedure is necessary.
- **Constraints:** SQL secondary UNIQUE/FK/CHECK constraints are not enforced by the domain. G15 refuses schemas with unsupported runtime constraint declarations.
- **Isolation:** A global optimistic conflict check and snapshot copies are used; this is not MVCC with configurable isolation levels.
- **Performance:** The domain scans and clones committed rows when beginning a transaction; no disk-first transaction snapshot/predicate locks are claimed.
- **Operations:** No multi-process SQL sessions, authentication, network protocol, query optimizer, multi-row WHERE, JOIN, aggregation, dynamic DDL, savepoints or online migration.
- **Backups:** G15 backs up the shared domain, but does not atomically capture its independently stored G11 catalog. A production database-wide backup still requires coordinated catalog/domain state publication.

## Source and verification

- `crates/bradian-relational/src/domain_sql.rs` — shared-domain SQL engine and scoped SQL sessions.
- `crates/bradian-relational/src/sql.rs` — reused parser/binder helpers and structured domain errors.
- `crates/bradian-relational/src/transaction_domain.rs` — one-WAL atomic multi-table row transaction.
- `crates/bradian-relational/tests/domain_sql.rs` — end-to-end multi-table SQL commit, restart, rollback, abort, update/delete, registration and legacy rejection.

Source and tests are committed to `main`. **They are not independently compiled or executed here.** Automatic GitHub CI remains disabled by owner request.
