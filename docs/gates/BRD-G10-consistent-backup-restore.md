# BRD-G10 — Consistent row-store backup and verified restore

**Source:** Bradian Database Systems Master SRS v1.1  
**Design:** [ADR 0015](../adr/0015-consistent-backup-restore.md)  
**Status:** Functional implementation and file-based tests committed to `main`; manual acceptance pending.

## Delivered

The native relational RowStore can now create a consistent physical backup and verify it against the authoritative row state using its normal restart recovery and index verification routines. Backups contain a CRC-protected versioned `backup.info` receipt, the committed `checkpoint.current`, matching immutable snapshot and derived B+tree, and the WAL up to the captured acknowledged LSN.

`backup_to` checkpoints before capture. `backup_current_generation` instead preserves an existing checkpoint together with later WAL frames. Both hold the source mutation mutex across copying so the backup cannot accidentally mix unrelated file generations. A new backup is published by rename only after independent staged recovery succeeds.

`RowStore::restore_backup` restores to a new path, never overwrites an existing database, and validates the copied manifest, BRPG pages, B+tree pointers, WAL replay, LSN and row counts before making the restored directory visible. The restored store can accept new WAL writes.

## Source tests committed

- [x] Complete physical backup/restore roundtrip with inserts, deletes and atomic batches.
- [x] Backup isolation from later source modifications.
- [x] Committed checkpoint plus newer WAL replay during restore.
- [x] Reject existing backup or restore destinations without overwriting data.
- [x] Reject corrupted backup receipt.
- [x] Reject corrupted authoritative physical snapshot.
- [x] Reject corrupted WAL frame after the checkpoint.
- [x] Reject backup descriptor with regressed LSN.
- [x] Reject backup destination inside active database root.
- [x] Serialize backup and concurrent source writers into a consistent state.
- [x] Refuse checkpoint-free current-generation backup.

The checkboxes mean regression tests exist in the source, **not** that they have passed execution.

## Manual acceptance requirements

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

A production backup subsystem additionally requires large-volume throughput benchmarks, concurrent-writer stress, storage failure and power-loss injection, platform-specific rename/sync validation, full catalog and configuration coverage, secret/tenant authorization, encrypted remote storage, incremental backup retention and tested PITR recovery.

## Non-goals

This is not online lock-free backup, cluster-wide distributed consistent snapshots, database-wide PITR, encrypted backups or a transactional SQL catalog backup. The authoritative native row store remains single-node, and the independent logical catalog is not yet durably persisted. Automatic CI remains disabled.
