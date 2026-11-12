# ADR 0015 — Consistent native backup and verified restore

**Product:** Bradian Database Systems · Master Architecture & SRS v1.1  
**Status:** Source and file-based regression tests committed directly to `main`. Manual compilation, fault injection and platform qualification are still required.  
**Dependencies:** [ADR 0008 — WAL](0008-native-row-wal-checkpoint.md), [ADR 0010 — physical B+tree](0010-checkpoint-btree.md), [ADR 0013 — index recovery](0013-derived-index-reconstruction.md), [ADR 0014 — checkpoint manifest](0014-atomic-checkpoint-manifest.md).

## Problem

A valid Bradian local database is not merely a snapshot file. It consists of an explicitly committed `BRCP` checkpoint generation, a checksummed `BRPG` physical row snapshot, the matching derived B+tree, and zero or more subsequently synchronized `BRWL` mutation frames. Independently copying those files without coordinating with checkpoint publication and writers can create an unrecoverable combination of metadata and data.

The initial backup/restore milestone must produce a **recoverable point-in-time logical state at a specific acknowledged LSN**, reject incomplete or corrupt files, and never overwrite an existing database. Backup does not currently include the separate nonpersistent generic relational catalog, user credentials or a complete cluster configuration; those remain future requirements.

## Decision

Introduce `bradian-relational::backup` and two integrated RowStore methods:

- `RowStore::backup_to(destination)`: establish a new committed checkpoint and capture the complete recoverable database state under the RowStore mutex.
- `RowStore::backup_current_generation(destination)`: capture the already committed checkpoint together with every WAL frame acknowledged since that checkpoint, without forcing a new checkpoint. This is useful for repeated backup operations while the local database remains active.
- `RowStore::restore_backup(backup, destination)`: verify a captured backup and restore it into a **new, absent** database directory.

The backup contains only the files that define the chosen physical generation: `checkpoint.current`, `snapshot-<lsn>.brp`, `index-<lsn>.bri`, `rows.wal`, and the Bradian backup descriptor `backup.info`. Historic checkpoints, uncommitted staging files, `.orphan` quarantined files and the active source lock file are excluded.

## Backup descriptor

`backup.info` is a fixed-size, versioned 40-byte binary record, independent of the Rust memory layout:

| Offset | Width | Meaning |
|---|---:|---|
| 0 | 8 | `BRBKv001` magic |
| 8 | 8 | Committed checkpoint LSN, little-endian |
| 16 | 8 | Latest acknowledged WAL LSN, little-endian |
| 24 | 8 | Row count at captured LSN, little-endian |
| 32 | 4 | Reserved zero bytes |
| 36 | 4 | CRC32 over bytes 0–35 |

A CRC detects accidental metadata corruption; it does **not** provide authenticity, encryption or protection against deliberate rewriting by an attacker. The descriptor rejects an LSN preceding the checkpoint, invalid length, unknown version and nonzero reserved fields.

## Backup sequence

1. Optionally publish a checkpoint through the existing G9 manifest protocol, including migration of a verified legacy checkpoint that has no manifest.
2. Acquire the RowStore's mutation mutex and confirm that the instance is not poisoned.
3. Synchronize the current WAL while holding the mutex. The checkpoint LSN, latest LSN, row count, manifest, snapshot, index and log are now one consistent recoverable capture.
4. Require an absent destination directory outside the active database. Create a sibling `.bradian-incomplete` staging directory. Do not silently reuse an existing incomplete backup.
5. Copy each authoritative file into that staging directory and synchronize it. Write/synchronize the `backup.info` descriptor and synchronize staging-directory metadata on Unix.
6. Open the **copied** database in staging via the actual `RowStore::open` recovery path, verify that its replayed LSN and row count equal the recorded receipt, and run a full index-to-snapshot pointer verification. For a live backup, also compare the recovered row/value map with the locked source map. Any disagreement prevents publication.
7. Rename the verified staging directory to the requested destination and synchronize its parent on Unix. Release the source mutex.

A successful backup therefore contains one tested, recoverable state rather than a collection of apparently matching filenames. Writes after the backup mutex is released do not alter the captured directory. The backup itself can be reopened independently of the source.

## Restore sequence

1. Require a source backup directory, a valid `backup.info`, and an absent destination. Existing data must not be overwritten.
2. Copy exactly the four generation files and descriptor into a new sibling staging directory. Source backup files remain unchanged.
3. Reopen the staged database using its actual WAL and BRPG validation logic. Verify the manifest-selected generation, checkpoint index integrity, complete WAL replay, expected latest LSN, and expected row count.
4. Publish the staging directory only after all validation succeeds, by rename and parent-directory sync on Unix.
5. Any validation failure leaves the requested destination absent. A partial staging directory is deliberately retained for inspection rather than presented as a usable database.

Restored databases can then be reopened through the normal RowStore API and accept new WAL commits continuing from the restored LSN.

## Safety and operational limitations

- The backup is a **local single-RowStore physical backup**, not a distributed consistent snapshot, object-store backup, incremental PITR, logical SQL dump or full Bradian cluster disaster-recovery solution.
- `backup_to` forces a checkpoint; `backup_current_generation` does not. Both serialize source mutations during file copying, so large backups can block writers. Online nonblocking backup needs a separate version-pinning and WAL-retention design.
- No encryption, compression, remote transfer, key rotation, tenant authorization or backup retention policy is provided. Backups contain raw stored values; use only trusted, access-controlled destinations.
- CRC32 is corruption detection, not cryptographic authenticity. Source, backup and restore files must remain within a trusted filesystem and permissions model.
- Linux/Unix sync calls are present; macOS, Windows and all storage/filesystem power-loss guarantees require explicit fault-injection qualification.
- Backup scope currently excludes the independent transient relational catalog and Bradian Studio state. A full database product backup must include stable transactional catalogs, encryption keys through secure key escrow, configuration, users, and cluster metadata.
- Backup paths require an existing parent and cannot reside inside the active store. A failed staged backup is not overwritten automatically.

## Evidence

`crates/bradian-relational/src/backup.rs` implements versioned descriptors, staged file copying, verification and new-directory restore. `RowStore::backup_to`, `backup_current_generation`, and `restore_backup` coordinate source locks and checkpoint selection.

`crates/bradian-relational/tests/backup_restore.rs` covers complete roundtrip, source writes after capture, explicit checkpoint-plus-WAL replay, existing destination refusal, descriptor corruption, snapshot CRC failures, post-checkpoint WAL corruption, format validation, inside-source backup refusal, missing-checkpoint rejection and concurrent source writers.

Automatic CI is disabled per owner request, and no Cargo test pass is claimed. Manual testing and platform fault injection are mandatory before a production backup/recovery release.
