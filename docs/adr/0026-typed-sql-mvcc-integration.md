# ADR 0026 — Opt-in typed SQL execution over pinned MVCC versions

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** G21 Rust source and integration tests committed on `main`; Cargo build, test execution, WAL fault injection and operational qualification outstanding.  
**Dependencies:** [ADR 0016 — catalog](0016-persistent-ddl-catalog.md), [ADR 0020 — shared-WAL transaction domain](0020-shared-wal-multi-table-atomicity.md), [ADR 0021 — multi-table SQL](0021-shared-domain-sql-integration.md), [ADR 0023 — lifecycle](0023-catalog-domain-lifecycle-reconciliation.md), [ADR 0025 — MVCC foundation](0025-mvcc-row-version-foundation.md).

## Problem and decision

G20 introduced row version chains and per-key first-committer-wins optimistic validation, but SQL continued to use G15's global-LSN conflict policy. G21 binds the existing **validated G15 physical row format** and **G11 durable relational catalog identities** to G20's version engine and reuses G16's existing SQL grammar. It adds a new explicit **`MvccSqlEngine`** without changing the default G16 SQL engine or performing any unrequested in-place migration.

The implementation has two layers:

1. `typed_mvcc.rs` — `TypedMvccStore` opens a previously provisioned shared-WAL directory with G20 `MvccStore`, decodes the exact durable schema pin records and BRPK physical keys, validates each row against the pinned table schema, and maps typed `Value` operations to binary MVCC key/value mutations. The transaction adapter `TypedMvccTransaction` provides table-aware `get`, `scan`, `insert`, `replace`, `delete`, `commit`, and `rollback`.
2. `mvcc_sql.rs` — `MvccSqlEngine` owns the recovered G11 persistent catalog and the validated typed MVCC store. Its scoped `with_session` callback creates independent SQL transactions through G20. The SQL frontend reuses G12/G16 `Parser`, `coerce`, `key_predicate`, `locate` and result types; it does not introduce divergent SQL parsing or a separate type system.

## Storage compatibility and ownership

This is an opt-in access **mode** for an existing G15/G16/G18 shared-domain storage root. G21 deliberately uses the same schema-pin namespace `\0bradian-domain-table-v1:` and typed row format `BRPK`/version 1, including stable TableIds and typed BRRW row encoding. No new persistent row format or per-table WAL is invented.

`MvccSqlEngine::open(root)` requires existing catalog and shared-domain directories, refuses nonempty legacy `tables/` storage, and rejects a shared domain without registered schema pins. It compares every stored table definition against the authoritative recovered catalog; all existing physical rows must decode under their pinned schema and reproduce their primary keys. Missing/orphaned or conflicting definitions fail closed.

If the G18 lifecycle directory exists, G21 briefly opens the G18 coordinator **before** taking catalog/domain ownership, allowing it to complete interrupted `Pending` schema registration and reject missing active pins. It releases these temporary locks before opening the G21 engine. While G21 is running, its exclusive catalog and shared-domain RowStore locks prevent a second G16 engine or independent G18 coordinator from modifying the same physical files.

**Schema DDL is unavailable through G21.** Table registration remains a separate G15/G18 operation before opening G21; G21 neither auto-provisions new tables nor runs DDL concurrently with MVCC snapshots.

## SQL transaction model

`MvccSqlEngine::with_session` does **not** hold a global SQL-operation mutex. Different scoped sessions can overlap and each independently begin a pinned row-version snapshot via `BEGIN` or `BEGIN TRANSACTION`.

Supported DML remains the bounded G16 grammar:

| SQL | Semantics |
|---|---|
| `BEGIN` | Pin current shared-domain commit LSN and its historical visibility horizon |
| `SELECT ... WHERE full_primary_key` | Read a typed row as of the transaction start, plus its own staged changes |
| `SELECT ...` without WHERE | Scan an in-process snapshot of visible rows across the shared domain, filtering by table identity |
| `INSERT` | Validate every supplied column, field order, nullability, value type, primary key and physical row bound, then stage a binary row |
| `UPDATE ... WHERE full_primary_key` | Read transaction-visible row, validate replacement and preserve its primary key |
| `DELETE ... WHERE full_primary_key` | Stage a versioned deletion tombstone |
| `COMMIT` | Validate per-key versions, then synchronize all staged cross-table changes in exactly one BRWL batch |
| `ROLLBACK` | Discard staged changes and release the version pin without WAL I/O |

Every statement is fully parsed and validated; a statement error cancels the entire open transaction. Nested `BEGIN`, unsupported DDL, unqualified names, general WHERE expressions, joins and unknown/unpinned tables are rejected.

Per-key validation allows two transactions that began at the **same LSN** to commit when they change different physical keys, including keys belonging to different registered tables. Same-key conflicts (including delete/reinsert ABA while an old reader is pinned) reject the later commit. A transaction using a full `scan()` must pass G20's conservative global-LSN check before committing writes, preventing untracked full-scan phantoms but potentially increasing conflicts. COMMIT publishes a single synchronized G3 WAL batch, bounded to 256 operations and its existing 1 MiB body limit.

The active G20 store retains old row versions **in memory** while readers remain pinned. Old versions are reclaimed after they are no longer needed. Only current committed rows are checkpointed and recovered after restart: no historical time travel, cross-process version persistence, or SQL AS OF query is claimed.

## Concurrency and isolation caveats

- G21 is a **bounded snapshot-isolation foundation** for concurrent *in-process* SQL sessions, not full serializable isolation. Classic write skew across disjoint keys remains possible for transactions performing only point reads. Full scans use coarse global validation, not predicate locks.
- G21 does not allow concurrent independent **processes** to write the same database. Native RowStore file locks enforce one storage owner. All SQL sessions must share that owner.
- G21's pinned snapshots hold historical row bytes in RAM, with no spill-to-disk, admission control or session timeout. Full scans still traverse the in-memory version map.
- G11 catalog definitions remain pinned because their RowStore is exclusively owned by G21 during SQL execution; there is no online concurrent DDL or schema-version MVCC.
- G21 reuses G3 WAL's atomic batch and BRPG checkpoint protocol, but historical versions are not durable. Existing G19 database-wide backup requires relinquishing G21 ownership so the G18 coordinator can acquire the three file locks and checkpoint the latest committed state.
- Secondary UNIQUE/FK/CHECK row-level enforcement is not claimed and the existing shared-domain schema validator rejects unsupported declared constraints.
- No network SQL protocol, authentication, transaction scheduling fairness, serializable SQL, distributed isolation, versioned physical B+tree, PITR, optimizer, joins or arbitrary predicates is implemented.

## Integration example

```rust
use bradian_relational::mvcc_sql::MvccSqlEngine;

let engine = MvccSqlEngine::open(root)?;
engine.with_session(|session| {
    session.execute("BEGIN")?;
    session.execute("UPDATE banking.public.accounts SET balance=900 WHERE id=1")?;
    session.execute("INSERT INTO banking.public.movements(id,account_id,amount) VALUES(5,1,-100)")?;
    session.execute("COMMIT")?;
    Ok(())
})?;
```

An existing G16 database must be closed before opening G21. To resume G16 after G21, close the MVCC engine; the latest WAL-backed data remains readable in the original format.

## Source and release verification

Implementation: `crates/bradian-relational/src/typed_mvcc.rs`, `src/mvcc_sql.rs`. Tests: `crates/bradian-relational/tests/mvcc_sql.rs`. Existing G20 `src/mvcc.rs` and G15 typed codecs are reused without changing their durable format.

Regression source includes multi-table SQL transaction/restart, G16 format interoperability, stable old snapshots while another SQL session writes, truly concurrent disjoint-key SQL updates, same-key conflict, conservative scan conflict, rollback and failed-statement abort, legacy storage refusal, missing registration refusal, historical version cleanup, G18 Pending reconciliation, exclusive owner protection and multiple pinned read-only SQL sessions.

**Source has been committed to GitHub `main`, but compilation, test execution, Clippy and crash/power-loss qualification have not yet been performed.** Automatic GitHub Actions remain manual-only under the owner's instructions.
