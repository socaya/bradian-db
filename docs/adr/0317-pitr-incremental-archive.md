# ADR 0317 — PITR / incremental archive / offsite backup

## Status

Accepted. Promotes the earlier skeleton (0308-0319 pass) to
a **real end-to-end implementation**: reads WAL bytes,
chunks them with CRC32, writes a durable JSON manifest,
verifies, and applies back onto a fresh WAL that a real
`RowStore::open` replays into the archived state.

## Context

ADR 0014 already pins the row store's publication rule
(sync BRPG + marker before WAL truncation). Gate 1 ships
consistent whole-database backups (ADR 0015 / ADR 0024)
but has no **incremental** path: a nightly backup rewrites
the full store even when only a small WAL tail is new.

This ADR adds an incremental-archive primitive sitting at
the WAL byte-stream layer. It does not care about WAL
frame internals — the store already knows how to parse
them on reopen. The archive's only job is to preserve the
byte sequence verbatim across a chunking + offsite hop.

## Decision

New module `bradian_relational::incremental_archive`
exposing:

```rust
pub fn write_from_wal(
    wal_path: &Path, from_wal_offset: u64,
    archive_id: impl Into<String>, out_dir: &Path,
    options: ArchiveOptions,
) -> ArchiveResult<(ArchiveManifest, ArchiveReceipt)>;

pub fn read_manifest(archive_dir: &Path)                     -> ArchiveResult<ArchiveManifest>;
pub fn verify       (archive_dir: &Path)                     -> ArchiveResult<ArchiveReceipt>;
pub fn apply_to_wal (archive_dir: &Path, target_wal: &Path)  -> ArchiveResult<ArchiveReceipt>;
```

The manifest version-1 JSON lists:
`archive_id`, `wal_file_name`, `from_wal_offset`,
`to_wal_offset`, `total_bytes`, and a `chunks` array where
every chunk records its `index`, `[start_offset,
end_offset)` range, `crc32`, and chunk file name.

Invariants:

- **Write**:
  - Chunk size is the caller-supplied cap
    (default 16 MiB, `ArchiveOptions::chunk_bytes`).
  - Each chunk is `fsync`'d on write; the manifest is
    `fsync`'d after chunks.
  - Target directory must be absent or empty — never
    silently overwrite (ADR 0014 rule).
  - `from_wal_offset > wal_len` is refused.
  - Returns a receipt with the exact byte range covered.

- **Verify**:
  - Every chunk is re-read, length-checked and CRC32
    verified against the manifest.
  - Chunks must form a contiguous `[from, to)` range
    with no gaps.
  - Fails closed on the first mismatch.

- **Apply**:
  - Calls `verify` first — a corrupted archive NEVER
    reaches the target WAL.
  - Target length must equal `manifest.from_wal_offset`
    before appending. A full restore requires an absent
    target file (the call creates it); an incremental
    requires the target length to match the base archive
    boundary exactly, so holes and double-apply are
    impossible.

## Scope

**In scope for this slice:**
- Full WAL archive (`from_wal_offset == 0`) and
  incremental (`from_wal_offset > 0`).
- Local-filesystem output (bytes the operator then
  copies / rsyncs / uploads to object store).
- Restore into an empty target dir OR append onto an
  aligned existing `rows.wal`.
- End-to-end test: open a RowStore, write 100 rows, take
  base archive at row 50 and incremental for rows 50-99,
  restore both into a fresh dir, reopen, and confirm
  every row and the `last_lsn` match.
- Corruption detection: flip a byte; verify refuses and
  apply writes nothing.
- Misalignment detection: apply onto a target WAL of the
  wrong length fails without side effects.

**Explicitly out of scope** (follow-ups):
- Offsite transport (object store adapter).
- Encryption of chunks at rest.
- Delta reconstruction below the WAL layer (e.g. PITR
  AS OF a specific LSN) — ADR 0102's time→LSN map
  stays as the lookup oracle.

## Consequences

- An operator can take nightly base + hourly incremental
  archives of a RowStore directory and restore them end
  to end.
- 6 unit tests exercise the real round-trip, the
  corruption-detection path, misalignment refusal, and
  the write-side guards (non-empty target, offset past
  end).
- The module uses no new deps — `crc32fast`, `serde_json`,
  `thiserror` are already in the workspace.
- Removes the 0317 "skeleton" status per the 2026-11-24
  feedback.
