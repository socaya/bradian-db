# BRD-G30 — Catalog-bound durable DDL coordination fences

**Baseline:** Bradian Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/durable_typed.rs`, `history_sql.rs`  
**Status:** Source and four regression tests committed to `main`; independent Cargo build and execution outstanding.

## Delivered bounded implementation

G30 introduces an inactive DDL **coordination fence**, tied to both the G28 prepared proposal and the current G11 catalog WAL sequence. It does NOT activate ALTER, update the catalog or claim atomic commits across the two WALs.

- [x] Reserved `\\0bradian-ddl-coordinate-v1:` namespace in the existing G22 historical version-key WAL, followed by a stable 8-byte TableId.
- [x] Durable JSON record with strict fields: TableId, exact G28 proposal commit LSN, G11 catalog LSN, CRC32 of the proposed schema payload.
- [x] `HistorySqlEngine::coordinate_prepared_schema("db.schema.table")` captures the locked authoritative catalog WAL LSN and commits an immutable coordination record.
- [x] The transition requires a compatible previously prepared schema. Missing, cancelled or duplicate coordination attempts fail before any WAL publication.
- [x] Restart reconstruction validates key identity, exact proposal LSN, matching proposal bytes, chronology, immutable one-record-per-table semantics and refuses malformed/deleted/replaced records.
- [x] `HistorySqlEngine::open` cross-checks every recovered coordination record against the current G11 catalog WAL horizon. Any unexpected independent catalog mutation prevents SQL engine opening.
- [x] Coordination records are excluded from current and historical typed-row scans, so normal SQL continues to use the original immutable catalog.
- [x] G24 verified historical backup/restore retains coordination records automatically in the existing history component.
- [x] Four source regression tests: checkpoint/restart, missing or aborted proposal refusal, G24 restore, and catalog LSN divergence on reopen.
- [x] No new WAL file, no changes to original row formats and no CI workflow changes.

## Example

```rust
let db = HistorySqlEngine::open("./history-db")?;
// First, create a valid G28 prepared nullable-column proposal:
let fence_lsn = db.coordinate_prepared_schema("bank.public.accounts")?;
```

The operation writes only a coordination record, not a live schema revision. Its `proposal_crc32` detects accidental payload mismatch but is not a cryptographic signature.

## Recovery behavior and limitations

- **Prepared only:** the original catalog definition and row encoding remain active. An inactive coordination fence must not be interpreted as a successful `ALTER TABLE`.
- **Abort after coordination:** a subsequent G29 abort may still be recorded; this does not activate the proposed schema.
- **No silent reconciliation:** a catalog LSN mismatch fails closed. An operator must inspect the underlying independent journals; this implementation does not automatically roll forward or roll back catalog changes.
- **No cross-WAL atomicity:** G11 and G22 are separate sync domains. True DDL activation requires an explicit, recoverable multi-phase protocol and versioned row decoding.
- **Backup and retention:** G24 retains the record with historical versions. Full retention-policy behavior and power-loss injection remain to be qualified.
- **Performance:** proposals and retained versions are inspected using owned in-memory snapshots; high-volume histories need bounded streaming and benchmarks.

## Required manual validation

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

These commands have **not been executed** in the available runtime because Cargo/rustc are absent. Source commits and test files are not evidence of passing integration tests or production readiness.

**G30 is a recoverable DDL coordination checkpoint, not transactional schema activation.**
