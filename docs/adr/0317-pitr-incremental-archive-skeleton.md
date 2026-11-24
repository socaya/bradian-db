# ADR 0317 — PITR / incremental archive / offsite backup: skeleton

## Status

Accepted as a skeleton. Tenth slice of P2.

## Context

ADR 0102 already provides time → LSN lookup. The P2
acceptance calls for **incremental archive + offsite
backup**: given a "last archived LSN", produce a chunk
manifest over the next range of WAL, each chunk CRC-stamped
so an offsite reader can verify integrity.

## Decision

New module `bradian_relational::incremental_archive` with
typed manifest + chunk + builder:

```rust
pub struct ArchiveChunk { pub min_lsn: u64, pub max_lsn: u64, pub crc32: u32, pub bytes_len: usize }
pub struct ArchiveManifest {
    pub archive_id:   String,
    pub min_lsn:      u64,
    pub max_lsn:      u64,
    pub chunks:       Vec<ArchiveChunk>,
    pub total_bytes:  u64,
}
pub struct ArchiveBuilder { ... }
impl ArchiveBuilder {
    pub fn new(archive_id: impl Into<String>, from_lsn: u64) -> Self;
    pub fn add_chunk(&mut self, max_lsn: u64, payload: &[u8]) -> ArchiveResult<()>;
    pub fn finish(self) -> ArchiveManifest;
}
```

Honest scope:

- Builder validates monotonic LSN progression; otherwise
  no WAL reading, no offsite transport, no encryption,
  no restore.
- `crc32` uses the existing `crc32fast` workspace dep.

## Consequences

- 3 unit tests: builder rejects non-monotonic chunk LSN;
  manifest min/max/total aggregate correctly; CRC
  differs for differing payloads.
