# ADR 0002 — `.derc` native segment format v1

**Status:** Accepted · 2026-11-11
**Spec sections:** §3.1–§3.3, §3.5, requirement STO-001/003/006

## Context

Spec §3.2 requires a portable, versioned binary format for immutable column segments that
supports: magic header; format major/minor; dataset/partition/segment IDs; schema fingerprint;
row count + row groups; column metadata; typed pages; codec IDs; null/validity bitmap; dictionary
metadata; zone maps; compressed buffers; payload offsets; per-page checksums;
authentication/encryption metadata; feature flags. Unknown critical features must fail closed.

## Decision — file layout v1

All multi-byte integers are little-endian. All offsets are absolute file offsets. All lengths
are in bytes.

```
+---------------------------------------------------+
|                 FILE HEADER (64 B)                |
+---------------------------------------------------+
|   magic [8]       = "DERC\x01SEG"                 |
|   version_major u16  = 1                          |
|   version_minor u16  = 0                          |
|   feature_flags u64  = required-feature bitset    |
|   optional_flags u64 = optional-feature bitset    |
|   created_at    u64  = unix millis                |
|   writer_id     u64  = truncated BLAKE3           |
|   reserved     [16]  = 0                          |
+---------------------------------------------------+
|                    COLUMN PAGES                   |
|  (contiguous; column pages are keyed from footer) |
+---------------------------------------------------+
|                   DICTIONARIES                    |
|   per-column optional dictionary payloads         |
+---------------------------------------------------+
|                     FOOTER                        |
+---------------------------------------------------+
|   footer_magic [4]  = "DRCF"                      |
|   footer_len    u32  = length of bincode payload  |
|   footer_crc    u32  = CRC32(footer_payload)      |
|   footer_payload[]   = bincode(SegmentFooter)     |
+---------------------------------------------------+
|   last 8 bytes = footer start offset (u64 LE)     |
+---------------------------------------------------+
```

`SegmentFooter` (bincode) carries:

- segment identity: `dataset_id`, `partition_id`, `segment_id` (DHIS2 UIDs or opaque strings).
- schema fingerprint (BLAKE3 of the ordered column metadata list).
- row count, row group boundaries (byte + logical).
- `columns: Vec<ColumnMeta>` where each `ColumnMeta` carries:
  - `name`, `logical_type`, `physical_type`, `nullable`,
  - `pages: Vec<PageMeta>` with offset, compressed_len, uncompressed_len, row_count, codec,
    null_count, min/max zone map, per-page CRC32,
  - optional dictionary (offset + len + element count).
- segment-level zone map (merged min/max per column).
- ingest watermark (source system LSN / timestamp).
- metadata revision pointer (from `dap-catalog`).

### Required feature flags

Bit 0 — `COMPRESSED_PAGES` (zstd / lz4). Reader must support zstd.
Bit 1 — `DICT_ENCODING`.
Bit 2 — `BITPACK_ENCODING`.
Bit 3 — `DELTA_ENCODING`.
Bit 4 — `PAGE_CRC32`.

Any **critical** bit the reader does not implement → error `UnsupportedSegmentFeature`. Optional
flags live in `optional_flags`; unknown optional bits are ignored.

### Codec IDs

| ID | Codec | Payload interpretation |
|---|---|---|
| 0 | Plain | fixed-width little-endian values |
| 1 | PlainUtf8 | length-prefixed UTF-8 strings |
| 2 | Dictionary | dictionary indices (bit-packed), payload references dictionary section |
| 3 | RunLengthEncoded | (run_length u32, value) pairs |
| 4 | Delta | first value + delta-of-delta zig-zag varints |
| 5 | BitPacked | fixed-width packed integers (width in page meta) |
| 100+ | Compressed wrapper | compound: outer codec = zstd or lz4; inner payload uses codec from `inner_codec` field in page meta |

Codecs are chosen by the writer using per-page statistics (cardinality, sortedness, null
fraction). The reader does not need to re-derive the choice.

### Validity bitmap

One bit per row, LSB-first per byte. Stored as a page-level separate buffer immediately before
the values payload. If `nullable=false` the bitmap is omitted.

### Per-page checksum

CRC32 (iSCSI polynomial via `crc32fast`) over the concatenation of the compressed value buffer
and, when present, the validity bitmap. Mismatch → error `SegmentCorruption`.

### Atomic publication

A segment file is first written to `segments/<id>.derc.tmp`, fsynced, then renamed to the final
name. The partition `manifest` records the generation + visible segment set and is itself
written via temp-and-rename with its own CRC32. Readers hold a snapshot handle referencing the
generation; compaction cannot delete files referenced by live snapshots.

## Consequences

- Readers can validate structural integrity without decompressing payloads.
- Footer-first scan means a manifest only needs to carry `(segment_id, path, size)`; everything
  else is self-describing.
- Writing is single-pass with a post-pass footer — no bookkeeping of absolute offsets up front.
- Any post-v1 breaking change bumps `version_major`; any additive change bumps `version_minor`
  and reserves a feature flag bit.
