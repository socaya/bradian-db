# BRD-G19 — Database-wide consistent backup and verified restore

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0024](../adr/0024-database-wide-consistent-backup.md)  
**Implementation:** `crates/bradian-relational/src/database_backup.rs` and `lifecycle.rs`  
**Status:** Source and regression tests committed to `main`; build, execution, crash injection and platform qualification remain open.

## Implemented slice

G19 captures a coherent single-node database backup by holding exclusive file locks for the G11 catalog, G15 shared-WAL domain, and G18 lifecycle journal through the G18 `CatalogLifecycle` coordinator. The source lifecycle is reconciled and the three components are copied by their existing G10 verified backup APIs. The top-level versioned JSON manifest binds their checkpoint LSNs, acknowledged WAL LSNs, row counts, active TableIds and exact component archive byte checksums.

Before publication, the backup reconstructs all three components into an unpublished verification root and opens the durable lifecycle coordinator to validate matching catalog definitions, shared-domain schema pins, active table identities, LSNs and row counts. Restore repeats full verification and publishes only to an absent directory.

- [x] `CatalogLifecycle::backup_database_to(destination)` coordinates all three durable components.
- [x] Coordinator operation mutex serializes backup, provisioning and reconciliation.
- [x] G10 individually verified snapshot/WAL archives for catalog, shared domain and lifecycle.
- [x] `BRADIAN-DATABASE-BACKUP-V1` manifest with per-component recovery receipts.
- [x] Streaming CRC32 for exact five-file archives, preventing accidental substitution.
- [x] Independent full-root recovery and reconciliation before backup publication.
- [x] `restore_database(archive, destination)` refuses an existing destination and verifies source archives.
- [x] Restored domain supports subsequent cross-table SQL changes.
- [x] Reject missing/damaged manifest, missing/altered component archives, overlapping destinations and leftover staging.
- [x] Source row values and logical metadata retained; source may publish physical checkpoints.
- [x] Existing manual-only GitHub Actions behavior preserved.

These checkboxes describe **implementation and source test coverage**, not observed passing test runs.

## Committed regression tests

`crates/bradian-relational/tests/database_backup.rs` covers complete three-component backup/restore, source divergence after capture, damaged metadata, tampered component files, missing component, existing destination refusal, source overlap/stale staging, exclusive coordinator ownership and new SQL writes after restore.

## Manual verification

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Production qualification must also include termination and power-loss injection between each G10 component checkpoint/archive, top-level manifest sync, staged verification and final rename; disk-full testing; permission errors; large backups; all supported filesystem rename/flush semantics; backup restoration drills and controlled operator cleanup of incomplete staging.

## Not yet implemented

- [ ] Lock-free online snapshot with concurrent DML/DDL and version-pinned catalogs.
- [ ] Encrypted, signed, authenticated, compressed, incremental or remote backups.
- [ ] Database point-in-time recovery across multiple component WAL histories.
- [ ] Cryptographic verification against a trusted external anchor.
- [ ] Cluster-consistent backup, distributed coordination or replication.
- [ ] Atomic cross-WAL catalog DDL transactions and full ALTER/DROP lifecycle.
- [ ] Qualified no-replace directory rename under concurrent hostile creators.
- [ ] Production performance benchmarks, fault-injection acceptance and supported-platform certification.

**G19 is a verified coherent single-node archive and restore for cooperating local owners, not a guarantee of distributed or fully online ACID backup.** Automatic CI remains disabled per owner direction.
