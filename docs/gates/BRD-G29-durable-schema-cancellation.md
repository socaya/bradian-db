# BRD-G29 — WAL-durable cancellation and recovery of schema proposals

**Baseline:** Bradian Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/durable_typed.rs`, `history_sql.rs`  
**Status:** Source and regression cases committed to `main`; independent Rust compilation, test execution and failure-injection qualification not demonstrated.

## Implemented bounded recovery step

G28 can prepare one immutable nullable-column proposal per table in the G22 historical WAL; it does not alter the G11 catalog. G29 introduces a durable terminal **ABORTED** decision for that proposal. This is not an active ALTER engine and does not pretend that separate catalog/history WALs form a single atomic transaction.

- [x] Dedicated `\\0bradian-schema-abort-v1:` namespace followed by stable 8-byte big-endian TableId.
- [x] The abort marker is inserted once as a G22 WAL-atomic `ABORTED-V1` value. It is not a physical deletion of the previously prepared proposal or historical data.
- [x] Aborting requires an existing prepared proposal; missing proposals and repeat abort attempts fail closed without a WAL write.
- [x] The stored proposal is decoded and checked against the immutable catalog schema and G27 nullable-column compatibility before the abort is committed.
- [x] Restart verifies every retained abort marker: exact key length, valid table identity, matching prepared intent, committed ordering strictly after the original proposal, exact payload and non-rewriting semantics.
- [x] Abort metadata is excluded from G23 typed row scans and SQL historical SELECT so it cannot be confused with user rows.
- [x] G24 backups capture and restore both the prepared schema and abort marker using the existing verified history WAL.
- [x] `HistorySqlEngine::abort_prepared_schema("db.schema.table")` returns the committed WAL LSN.
- [x] `HistorySqlEngine::aborted_schema_proposals()` lists recovered decisions **as observed when the engine opened**. Reopen to refresh.
- [x] Three source-level integration tests cover WAL/restart recovery, missing proposal refusal and G24 archive restoration.
- [x] No storage-format migration, CI changes or automatic workflow activation.

## Example

```rust
let db = HistorySqlEngine::open("./history-db")?;
let decision_lsn = db.abort_prepared_schema("bank.public.accounts")?;
```

This requires that G28 already prepared an additive nullable-column proposal for `bank.public.accounts`. After a successful abort the original G11 catalog and historical row encoding remain unchanged. A proposal is permanently retired by the decision marker; this version does not reuse the proposal key or support another attempt for that table.

## Deliberate limitations

This is **recovery-safe intent cancellation**, not transactional DDL activation. Real historical ALTER still requires recoverable catalog coordination, immutable schema epoch publication, row-to-epoch binding, multi-version row decoding, and fault-injection tests. A single historical WAL marker cannot atomically modify the separate G11 catalog.

The in-memory recovered proposal/abort inventory is a snapshot at engine open. The G22 store validates concurrent duplicate inserts; callers should reopen before interpreting recently changed status. Archive CRC32 detects accidental damage but does not authenticate malicious modification.

## Manual verification

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

All of these runtime checks remain outstanding; source commits and test cases are **not proof of passing tests or production readiness**.
