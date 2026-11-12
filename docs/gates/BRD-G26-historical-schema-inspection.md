# BRD-G26 — Immutable historical schema inspection

**Baseline:** Bradian Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/durable_typed.rs`, `history_sql.rs`  
**Status:** Source and regression cases committed to `main`; Cargo compilation, execution and operational qualification outstanding.

## Delivered compatibility-safe increment

G23 durable typed history already records each table's schema as one immutable, typed registration version in G22's WAL. G26 now exposes that verified historical registration information without altering data formats or creating misleading schema-evolution writes.

- [x] During history-domain open, extract each table's original schema-registration commit LSN from retained G22 versions.
- [x] Preserve strong fail-closed checks: rewritten/missing schema pins and corrupt retained typed versions cannot be accepted as valid schema history.
- [x] `DurableTypedStore::schema_as_of(TableId, LSN)` returns the immutable validated `Table` and its registration LSN, or refuses an unavailable or invalid LSN.
- [x] `DurableTypedStore::schema_epochs()` enumerates known table-registration LSNs.
- [x] `HistorySqlEngine::schema_as_of("db.schema.table", LSN)` and `schema_epochs()` make this data available to callers.
- [x] Read-only SQL `DESCRIBE TABLE db.schema.table AS OF <unsigned WAL LSN>` returns `column_name`, `data_type` (JSON-encoded type), `nullable` and `schema_registered_lsn` (decimal string to preserve full unsigned precision).
- [x] DESCRIBE works without BEGIN; no row mutation or WAL write is introduced by inspection.
- [x] Two regression cases committed for restart/checkpoint stability and invalid/pre-registration LSN refusal.
- [x] G15/G16/G18/G19 and G22/G23/G24/G25 physical formats unchanged; automatic CI remains disabled.

## Example

```sql
DESCRIBE TABLE bank.public.accounts AS OF 4;
```

Or through the Rust API:

```rust
let engine = HistorySqlEngine::open("./history-database")?;
let (table, registered_lsn) = engine.schema_as_of(
    "bank.public.accounts",
    engine.last_lsn(),
)?;
```

The LSN must be within the durable retention floor and last committed WAL position. The table must already have been registered at that LSN.

## Boundaries and next work

**This is read-only historical schema inspection, not general schema evolution.** A G23 table still has exactly one immutable definition. `ALTER TABLE`, `DROP COLUMN`, epoch migration, mixed historical row layouts, catalog/row atomic DDL, old-column value decoding and backward-compatible historical projections remain unimplemented.

A future versioned-schema write path must first define WAL-atomic schema transitions (or coordinated catalogs), row-version-to-schema binding, retained epoch garbage collection, compatibility rules for indexed PK encodings, and deterministic recovery tests. Do not reinterpret old G22 schema pins as mutable DDL.

## Manual verification

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

These commands have **not been successfully executed in this environment**. Source/test commits are not evidence of passing builds or production certification.
