# ADR 0121 — Incremental backup delta (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch K K1 — partial close
of BRD-DR-001 PITR item.

## Scope

- `incremental_backup::write_delta(source_wal, dest_dir,
  base_lsn)` scans the WAL, copies only frames with
  `lsn > base_lsn` into a `delta-<base>-<tip>.wal` file,
  writes `delta.manifest` JSON.
- `read_delta(path)` verifies magic + version + CRC32,
  returns `(base_lsn, tip_lsn, frame_bytes)`.
- `DeltaManifest { base_lsn, tip_lsn, frame_count, bytes,
  created_at_ms }`.

## Out of scope

- **Delta apply / restore** — reader is provided; the WAL-
  replay side of restoring base-plus-N-deltas is a follow-
  up.
- **Checkpoint pages in delta** — only WAL frames.
  Operators retain the full base backup for checkpoint
  state.
- **Compression / encryption** — plain bytes.
- **Delta cleanup on quota** — operator manages retention.

## Compliance

- Delta captures only frames above base_lsn.
- Empty delta rejected (nothing to write).
- Read-after-write preserves bytes.
- CRC tamper rejected.
- 4 unit tests.
