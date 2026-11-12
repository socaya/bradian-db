# Bradian G2 — Portable physical page format (pre-durability slice)

**Specification:** Bradian Master Architecture & SRS v1.1  
**Design decision:** [ADR 0007](../adr/0007-portable-physical-page.md)  
**Status:** Source committed; full gate remains open until the row-store and durability subsystems are implemented.

## What this slice delivers

The `bradian-relational::page` module serializes a fixed 4096-byte native page containing an explicit magic, format version, kind, page ID, log sequence field, payload length and CRC32 checksum. Decoding rejects an unknown format, flags, page kind, noncanonical padding, wrong length, oversized payload or corruption.

Regression tests cover byte order, page categories, boundary payload sizes and mutation of header and payload bytes. The existing analytical `.derc` segments and DHIS2 adapters are not modified.

## What this slice does not deliver

No persistent data file, buffer cache, slotted-row records, B+tree, free-page allocator, write-ahead log, checkpoint, crash consistency, file encryption, multi-process locking or ACID transactions has been implemented. The page's LSN is an informational field until WAL ordering is implemented. A CRC is not authentication.

## Verification

- [ ] `cargo check --workspace --locked`
- [ ] `cargo test --workspace --locked`
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings`
- [ ] `npm run build` for Bradian Studio
- [ ] Windows, macOS and Linux byte-for-byte page compatibility checks
- [ ] Torn-write/power-loss recovery tests (requires WAL integration)
- [ ] Independent security review of future page encryption and key custody

Do not mark the full G2 storage gate complete after the page-codec tests alone.
