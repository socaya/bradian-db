# Checkpoint and archive metadata limits

This fixed Master Gate 1 / G1-MEMORY policy bounds metadata before parsing or
publication. The existing physical formats and CRC conventions are unchanged.

| Metadata | Bound | Failure contract |
|---|---|---|
| checkpoint.current | Exactly 40 bytes, plus one trailing-byte probe | Short/oversized records are manifest corruption; absent file remains optional |
| backup.info | Exactly 40 bytes, plus one trailing-byte probe | Short/oversized records are descriptor corruption |
| history-backup.json | Default 1048576 serialized bytes | Over-policy reads/writes are resource errors |
| database-backup.json | Default 1048576 serialized bytes | Over-policy reads/writes are resource errors |

Binary records use fixed arrays. JSON reads retain at most policy bytes and consume
at most one extra byte to detect overflow, using an 8 KiB scratch buffer. They do
not trust a file-size check, so growing files cannot bypass the byte ceiling. Short
and interrupted reads are handled. JSON parsing begins only after bounded input.
Malformed JSON below policy remains corruption; actual I/O errors remain I/O errors.

## Native policy

runtime_limits::MetadataLimits has max_manifest_bytes. Existing APIs use its default.

- HistorySqlEngine::backup_to_with_metadata_limits(destination, metadata) configures
  historical archive serialization.
- history_backup::restore_history_database_with_policies(archive, destination,
  history_limits, metadata) composes retained-history and manifest-input policy.
- CatalogLifecycle::backup_database_to_with_metadata_limits(destination, metadata)
  configures lifecycle archive serialization.
- database_backup::restore_database_with_metadata_limits(archive, destination,
  metadata) configures lifecycle archive input.

Binary format lengths are mandatory and cannot be raised with MetadataLimits.
Zero JSON bytes is a valid policy but cannot admit a nonempty valid archive manifest.
A valid larger JSON manifest may be admitted by explicitly raising policy. Policies
are per call and never persisted. Existing SQL/CLI flags do not configure this policy.

## Writes, verification and recovery

Canonical CRC calculation streams serialized payload bytes instead of retaining a
second payload vector. Serialization borrows the report/table list and checks the
complete envelope's bytes before creating its manifest file. A policy failure cannot
publish an archive that its matching read policy would refuse. Field order, escaping,
integer encoding and CRC bytes retain their existing conventions.

Read refusal occurs before component CRC work and restore destination staging. It
never changes archive/source bytes. Write refusal occurs before manifest-file creation
and final archive publication, but component backups/checkpoints may already exist in
an incomplete stage. This preserves the existing checkpoint-then-backup and explicit
cleanup/fresh-destination contract; no automatic stage deletion is added. Source
logical rows remain intact, and the owner can continue valid work.

This limits serialized buffers, not total RSS. Allocator/vector slack, report/catalog
objects, decoded JSON structures, archive payload copying/hashing, directory inputs,
active transaction totals and global admission remain separate. No cancellation,
failed I/O, OOM, disk-full, power-loss or cross-platform qualification is implied.

See [ADR 0048](../adr/0048-bounded-archive-metadata.md) and
[executed verification](../implementation/BRADIAN_GATE1_METADATA_LIMITS_VERIFICATION_2026_11_13.md).
