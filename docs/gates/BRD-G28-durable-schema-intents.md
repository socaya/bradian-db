# BRD-G28 — Durable prepared schema transition intents

**Architecture:** Bradian Master Architecture & SRS v1.1  
**Implementation:** `crates/bradian-relational/src/durable_typed.rs`, `history_sql.rs`  
**Status:** Source and three regression tests committed to `main`; Cargo build, execution, failure injection and release qualification outstanding.

## Implemented and deliberately bounded

G28 establishes a durable **PREPARED** state for additive nullable schema revisions without enabling a second, unsynchronized source of truth. G11 catalog DDL and G22 historical versions currently use separate WALs, so an actual online `ALTER TABLE` must not be simulated using a unilateral catalog or history mutation.

- [x] New namespace `\\0bradian-schema-proposal-v1:` followed by an 8-byte big-endian TableId inside G22's existing single history WAL. **The old G22 physical history format is unchanged.**
- [x] `DurableTypedStore::prepare_nullable_addition(table_id, proposed)` verifies the authoritative immutable schema and G27 append-only nullable compatibility, then commits one durable proposal version.
- [x] A duplicate proposal for the same TableId is rejected through snapshot-visible unique key insertion.
- [x] On restart, every retained proposal key, payload, registration order, and compatibility relationship is revalidated. Rewritten, deleted, malformed, unregistered or incompatible proposals fail closed.
- [x] Existing SQL transactions, `SELECT ... AS OF`, `DESCRIBE ... AS OF`, and typed row scans exclude the proposal namespace.
- [x] `HistorySqlEngine::prepare_nullable_addition(...)` and `prepared_schema_proposals()` expose preparation and recovered intent inspection. The proposal listing reflects the opening snapshot; reopen the engine to inspect newly prepared intents.
- [x] G24 backup/restore captures prepared proposals in the normal historical WAL and validates them on restored-root open.
- [x] Three source regression tests cover checkpoint/restart recovery, incompatible change refusal, and verified G24 restore.
- [x] No automatic CI or workflow changes.

## Example

```rust
use bradian_relational::{Column, DataType};
use bradian_relational::history_sql::HistorySqlEngine;

let db = HistorySqlEngine::open("./history-db")?;
let (current, _) = db.schema_as_of(
    "bank.public.accounts", db.last_lsn()
)?;
let mut proposed = current.clone();
proposed.columns.push(Column {
    name: "memo".into(),
    data_type: DataType::Text,
    nullable: true,
});
let prepared_lsn = db.prepare_nullable_addition(
    "bank.public.accounts", &proposed
)?;
```

**The new column is NOT active.** SQL SELECT/INSERT/UPDATE continue to operate with the original authoritative G11 schema. A prepared intent is neither committed ALTER nor a second active schema epoch. No automatic roll-forward, abandonment or schema-switch API is exposed.

## Remaining milestones

1. Define and persist a recoverable catalog/history DDL transaction protocol with idempotent intent phases, catalog ownership and crash-safe publication.
2. Introduce actual historical schema epochs and bind physical row versions to the correct epoch, including mixed old/new row layouts.
3. Implement an authenticated operator-controlled activation path, rollback/refusal rules and destructive crash-injection tests.
4. Add retention and proposal lifecycle policies; prevent old schema pin removal while any row version depends on it.
5. Qualify bounded-memory retained-history verification, table-wide migration and disk-full behavior.

## Manual verification required

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

**These commands have not been executed successfully in the current environment.** The committed changes are an opt-in experimental DDL preparation foundation, not production-qualified transactional DDL.
