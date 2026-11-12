# BRD-G27 — Additive schema evolution compatibility and historical projection

**Baseline:** Bradian Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/schema_evolution.rs`, `history_sql.rs`  
**Status:** Implementation and tests committed to `main`; Rust build and operational qualification outstanding.

## Implemented bounded G27 foundation

G22/G23 historical rows retain an immutable schema pin and a typed row format. The G11 catalog is stored in a different WAL. Consequently, applying real DDL to one component without coordinated recovery of the other is unsafe. This milestone provides an executable, **read-only compatibility planner and historical row projection**. It does not publish schema changes or claim ALTER TABLE support.

- [x] `plan_nullable_additions(old, proposed)` validates both table definitions.
- [x] Identical TableId, SchemaId, table name, existing columns, primary key, UNIQUE and foreign-key definitions required.
- [x] Reject column drops, renames, reordering, type/nullability changes and non-nullable additions.
- [x] Permit one or more trailing nullable additions with no defaults or backfill requirement.
- [x] `project_row(plan, old_row)` validates the source typed row and appends NULL values for the new columns, without writing physical bytes.
- [x] `HistorySqlEngine::preview_nullable_additions_as_of(table_name, lsn, proposed_table)` decodes rows at a retained WAL LSN under the verified actual schema and projects them into the proposed shape.
- [x] Three unit tests for safe append, invalid schema changes and invalid row shapes.
- [x] One end-to-end source test for history projection after checkpoint/restart and unchanged source WAL/catalog.
- [x] No persisted format changes or automatic CI.

## Rust example

```rust
use bradian_relational::{Column, DataType};
use bradian_relational::history_sql::HistorySqlEngine;

let engine = HistorySqlEngine::open("./history-database")?;
let horizon = engine.last_lsn();
let (old, _) = engine.schema_as_of("bank.public.accounts", horizon)?;
let mut proposed = old.clone();
proposed.columns.push(Column {
    name: "comment".into(),
    data_type: DataType::Text,
    nullable: true,
});

let preview = engine.preview_nullable_additions_as_of(
    "bank.public.accounts", horizon, &proposed,
)?;
```

`preview` has the proposed number of columns; every added field contains `Value::Null`. The G11 catalog still contains the old schema.

## Important correctness boundaries

**Real schema evolution is not yet implemented.** G27 does not execute SQL ALTER, change G11 catalog events, update G22 immutable schema pins, assign a new schema epoch, accept new-column writes, or upgrade the row codec. The planner is deliberately conservative and can only show that a nullable trailing append is compatible with existing rows. Snapshot retention floors continue to apply.

Before enabling durable DDL, Bradian needs: a schema epoch format with atomic catalog/history publication or recoverable intents; epoch-bound physical row decoding; rollback and crash recovery tests; migration locks; explicit default/backfill behavior; and proven cross-version read/write compatibility.

## Manual checks

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

**None of these commands has been executed successfully in the current environment.** Committed tests are not certified passing tests.
