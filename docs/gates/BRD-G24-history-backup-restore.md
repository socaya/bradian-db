# BRD-G24 — Verified durable-history database backup and restore

**Specification:** Bradian Database Systems Master Architecture and SRS v1.1  
**Implementation:** `crates/bradian-relational/src/history_backup.rs`, `history_sql.rs`, `durable_typed.rs`  
**Status:** Implementation and three regression tests committed to `main`; build, test execution and operational qualification remain outstanding.

## Bounded implemented scope

The G23 historical SQL database owns a G11 persistent catalog and a G22 durable history domain. G24 introduces a separate `BRADIAN-HISTORY-BACKUP-V1` archive for precisely these two components. It never mixes the incompatible G19 shared-domain/lifecycle layout with historical storage.

- [x] `HistorySqlEngine::backup_to(destination)` holds the catalog and history-domain exclusive store ownership throughout archive creation and verification.
- [x] Two existing independently verifiable G10 component archives: `catalog/` and `history-domain/`.
- [x] Versioned `history-backup.json` manifest with catalog/history checkpoint and final LSNs, row receipts, ordered TableIds, retained-version count and durable retention floor.
- [x] Streaming CRC32 checksum over all five required files for each component; metadata envelope checksum.
- [x] Independent reconstruction and catalog/schema/version verification in unpublished staging storage.
- [x] `restore_history_database(archive, destination)` verifies checksum, G10 receipts and G23 schema history before publishing a new, absent destination.
- [x] Refuse archive/source overlap, existing destinations, unfinished staging and mixed legacy storage.
- [x] Preserve historical AS OF results, tombstones and the original source directory.
- [x] Three committed tests cover AS OF after restore, archive tampering and destination refusal.
- [x] No automatic GitHub Actions or CI changes.

## API example

```rust
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::history_backup::restore_history_database;

let engine = HistorySqlEngine::open("./history-db")?;
let manifest = engine.backup_to("./archive-2026")?;
drop(engine);
let recovered = restore_history_database("./archive-2026", "./restored-history-db")?;
assert_eq!(manifest, recovered);
```

The source owner should not perform catalog changes through competing processes. Archive checksums protect against accidental damage, not malicious replacement; there are no archive signatures or storage encryption.

## Required manual validation

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

These commands were **not executed successfully in this environment**; Cargo was unavailable and the remote repository could not be cloned. Checkboxes indicate committed source and test cases, not passing test results.

## Remaining G24 work

- [ ] Lifecycle-aware migration from G18-managed shared-domain databases into the separate G23 history root. The G23 migration still deliberately refuses G18 sources.
- [ ] Catalog-plus-history atomic schema DDL and online immutable historical schema epochs.
- [ ] Fault injection for power loss, disk full, torn WAL, interrupted staging, metadata corruption, source-lock contention, rename races and restore drills.
- [ ] Streaming/bounded retained-version verification and operational limits for large historical databases.
- [ ] Cryptographically authenticated and encrypted archives; incremental/PITR, remote storage and multi-node consistency.
- [ ] Platform-qualified no-replace publication and filesystem durability guarantees.

**Gate interpretation:** G24 currently delivers a separate opt-in local historical archive/restore slice, not general lifecycle migration, certified recovery, or production-grade backup.
