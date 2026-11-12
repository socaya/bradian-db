# ADR 0024 — Coordinated database-wide backup and restore

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** G19 source and regression tests committed to `main`; compilation, test execution, and filesystem power-loss qualification remain outstanding.  
**Dependencies:** [ADR 0015 — single-store backup](0015-consistent-backup-restore.md), [ADR 0016 — persistent catalog](0016-persistent-ddl-catalog.md), [ADR 0020 — shared-WAL domain](0020-shared-wal-multi-table-atomicity.md), [ADR 0023 — durable table lifecycle](0023-catalog-domain-lifecycle-reconciliation.md).

## Problem

A recoverable Bradian SQL database is no longer a single RowStore directory. It comprises three distinct durable components: a G11 catalog journal, a G15 shared-WAL domain of registered table schemas and row values, and a G18 lifecycle journal reconciling their identities. G10 can back up each component independently but cannot, by itself, prove the copies represent a coherent database-wide state. Copying them individually while another component is being modified can produce inconsistent table schema pins or lifecycle states.

A database-level backup must capture all three while a single owner prevents **cooperating** writers from altering their logical state, then independently verify the recovered catalog, domain and lifecycle before publishing the archive.

## Decision

Extend `CatalogLifecycle` with `backup_database_to(destination)` and introduce `bradian-relational::database_backup::restore_database(backup, destination)`.

`CatalogLifecycle` already retains the three G11/G15/G18 RowStore exclusive file locks. G19 additionally uses a coordinator operation mutex around `provision_existing`, `reconcile` and `backup_database_to`, preventing those operations from interleaving within the coordinator. A full backup calls `reconcile_locked` before capture so all lifecycle entries are `Active`, every schema pin matches the catalog, and any interrupted `Pending` registration is resolved.

The destination is a **new separate directory**, never the active source, and must not exist. A sibling staging directory is created with a deterministic `.bradian-g19-incomplete` suffix and is not silently reused after failure.

## Archive structure

```text
<bradian-g19-backup>/
  database-backup.json      # version, three receipts, table identities, CRC32
  catalog/
    backup.info             # G10 catalog checkpoint/WAL receipt
    checkpoint.current
    snapshot-<lsn>.brp
    index-<lsn>.bri
    rows.wal
  shared-domain/
    backup.info             # G10 shared domain checkpoint/WAL receipt
    checkpoint.current
    snapshot-<lsn>.brp
    index-<lsn>.bri
    rows.wal
  lifecycle/
    backup.info             # G10 lifecycle checkpoint/WAL receipt
    checkpoint.current
    snapshot-<lsn>.brp
    index-<lsn>.bri
    rows.wal
```

The top-level `database-backup.json` contains a version marker `BRADIAN-DATABASE-BACKUP-V1`, ordered active TableIds, and a receipt for each component: checkpoint LSN, last acknowledged LSN, committed row count and an archive CRC32. The archive CRC32 streams the exact five G10 component filenames and bytes in a defined order; the enclosing payload is separately CRC32 checked. A swapped, truncated or accidentally damaged component is rejected before restore.

**CRC32 is not an authentication or confidentiality mechanism**. Actors able to edit files and recompute checksums could forge an apparently valid archive. G19 does not implement encrypted/signed backups or externally anchored hashes.

## Coordinated backup protocol

1. The application stops other G11/G15/G18 owners or allows the coordinator to acquire their exclusive file locks. The coordinator holds all three for the entire operation.
2. Under its operation mutex, the coordinator reconciles incomplete provisioning and checks current catalog/domain/lifecycle identity consistency.
3. Create a sibling unpublished staging directory after rejecting overlap, an existing destination, or a previous incomplete attempt.
4. Call G10 `backup_catalog_to`, G10 `MultiTableStore::backup_to`, and G10 `RowStore::backup_to` for the lifecycle journal. Each call checkpoints its source, synchronizes WAL, stages a verifiable single-store archive and publishes the component only after independent recovery verification. The source **may receive new physical checkpoints**; logical metadata and row values do not change.
5. Compute a CRC32 over each component's exact archive files, generate the database-level report, write/sync the versioned manifest, and synchronize its directory metadata.
6. Independently restore each component archive into a temporary `verified-root/` inside the unpublished staging tree. Open the recovered `CatalogLifecycle` and verify all active table identities, schema pins, exact component row counts and acknowledged WAL LSNs.
7. Close the verified copies, remove the temporary verification root, synchronize staging, rename the complete archive directory to its absent final destination and synchronize its parent directory on Unix.
8. Keep the source owner and its locks until publication completes or the backup returns an error.

An interrupted backup never publishes the requested destination while incomplete. Staging is retained for investigation rather than silently reusing possibly inconsistent files.

## Restore protocol

1. Verify the backup directory and parse the top-level versioned, CRC-protected manifest, requiring ordered unique table identities and nonregressing component LSNs.
2. Recompute all three component archive CRCs *before* creating the destination's staging root. A mismatched component fails without publication.
3. Restore each component using G10's verified RowStore restore into a fresh staging root, validating the component's individual receipt, checkpoint manifest, BRPG page CRCs, derived B+tree pointers and WAL replay.
4. Open and reconcile the complete G18 `CatalogLifecycle`. Require all tables active, exactly the reported stable TableIds, and the three reported component LSNs and row counts. Refuse orphan domain pins, absent catalog objects, missing active pins or inconsistent schema definitions.
5. Synchronize the fully restored staging directory and publish it by rename into an absent target. The restored root has `catalog/`, `shared-domain/`, and `lifecycle/` and can be opened for G16 cross-table SQL transactions.

The G19 archive itself is **not** a live Bradian SQL database root; each component is a G10 backup artifact until restored.

## Guarantees and limitations

- A G19 archive is a **coordinated single-node database-wide logical cut for cooperating processes**. The coordinator holds all three source file locks and serializes its own provisioning/backup actions. This is not a distributed snapshot, a cluster transaction or a substitute for a maintenance window in the presence of noncooperating filesystem writers.
- Each component retains its own WAL and checkpoint; there is no newly invented atomic cross-store DDL protocol. G18's durable lifecycle reconciliation makes cross-component identity verification meaningful but does not provide atomic multi-store DDL.
- A backup may checkpoint all three sources, so its operation is not a byte-for-byte read-only copy of their physical WAL directories.
- Source backups, staging restoration and verification can be I/O-intensive and hold locks for long periods. G19 supplies neither online lock-free snapshots nor incremental remote backup, compression, deduplication, retention management or point-in-time recovery.
- No encryption, cryptographic signatures, access controls, offsite transfer, replication or cluster consistency is supplied; archives contain raw data.
- The ordinary `fs::rename` path checks destination absence before publication, but its overwrite/no-replace behavior and crash-time durability are platform-dependent. A hostile or noncooperating concurrent destination creator is not safely serialized by G19. A separately qualified no-replace atomic publish primitive and fault injection are required for production.
- A failed archive or restore leaves a staged directory for investigation. Operators must examine and remove/quarantine it manually before retrying.
- A restored database is independent: new SQL writes do not propagate to the old source or archive.

## Source and tests

Implementation is in `src/database_backup.rs` with `CatalogLifecycle::backup_database_to`, G10 archive reuse, stream-checksummed component manifests and independent full-root verification. `tests/database_backup.rs` covers complete backup/restore, source divergence after backup, damaged manifests and archives, component substitution, incomplete archive refusal, existing-destination protection, overlap and unfinished staging, lock contention and writable SQL after restore.

**No build or test passes are claimed.** Automatic CI remains disabled at the owner's request. Manual `cargo check`, unit/integration tests, Clippy, corruption and power-loss injection, and filesystem-specific rename/sync validation remain mandatory.
