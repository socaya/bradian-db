# ADR 0163 — Projection segment writer (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Composes with ADR
0159 `projector_worker::drain_once` as a concrete sink.
Each `CommittedProjection` is persisted to a self-
describing on-disk file that any future columnar
reader can open.

## 1. Scope

New module `projection_segment`:

- Constants: `SEGMENT_MAGIC = *b"BRPS"`, `SEGMENT_VERSION = 1`.
- On-disk format (little-endian; 22-byte header):
  ```
  MAGIC (4) | version (u16) | as_of_lsn (u64) |
  json_len (u32) | CRC32 (u32) | JSON body
  ```
- `write_segment(path, &CommittedProjection) -> u64`
  (returns bytes written, `sync_all` before return).
- `read_segment(path) -> CommittedProjection` —
  validates magic + version + CRC + length.
- `SegmentError` variants: Io, Serde, BadMagic,
  UnsupportedVersion, ChecksumMismatch, Truncated.

Added serde derives to `ColumnBatch` and `RowBatch` to
support JSON encoding.

## 2. Out of scope

- **Append-only segment directories / segment index**
  — one file per projection today; a future catalog
  bridge can register them.
- **Bloom filters / column min-max statistics** —
  JSON body only.
- **Compression** — ADR 0082 DictIndex32 is the
  future compressed form.
- **Secondary-index generation**.

## 3. Compliance

7 unit tests in `projection_segment::tests`:

- `write_and_read_round_trips`.
- `write_syncs_bytes_through_to_disk`.
- `corrupt_magic_rejected`.
- `bit_flip_in_body_detected_by_crc`.
- `unsupported_version_rejected`.
- `truncated_segment_rejected`.
- `as_of_lsn_preserved_across_round_trip`.

## 4. Honest status

**BRD-HTAP-004** now has a durable sink. The
scheduler (ADR 0164 companion) drives this writer on
every advance_projected. Catalog registration of
emitted segments is the one remaining gap.
