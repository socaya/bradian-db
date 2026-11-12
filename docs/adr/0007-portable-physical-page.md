# ADR 0007 — Portable native physical page envelope

**Status:** Implemented as a tested *codec only* (not a durable row store)  
**Reference:** Bradian Database Systems Master Architecture & SRS v1.1, native storage and crash-consistency requirements.

## Context and choice

Bradian's proprietary relational storage requires an operating-system-independent physical format that can be inspected and validated before a B+tree, WAL, row layout, or buffer pool is implemented. The new `bradian-relational::page` module introduces a fixed-size **4096-byte** page, a **32-byte** header, explicit little-endian integers, page version and kind, a 64-bit page identity, a 64-bit last-log-sequence field, a bounded payload length, and a CRC32 checksum. Every byte after the payload must be zero. Decoding refuses unknown major version, unknown kind, flags, incorrect size or checksum, invalid payload length and nonzero padding. The checksum covers the entire physical page with the checksum field temporarily zeroed. No raw Rust struct layout is serialized.

## Binary layout

| Offset | Width | Meaning |
|---|---:|---|
| 0 | 4 | Magic ASCII BRPG |
| 4 | 2 | Format major version 1, little-endian |
| 6 | 1 | Kind: 1 data, 2 index, 3 free map |
| 7 | 1 | Flags reserved and required to be zero |
| 8 | 8 | Page ID, little-endian u64 |
| 16 | 8 | Last-log-sequence number, little-endian u64 |
| 24 | 4 | Payload size, little-endian u32, maximum 4064 |
| 28 | 4 | CRC32 checksum of 4096 bytes with bytes 28–31 set to zero |
| 32 | Up to 4064 | Payload followed by zero padding |

## Safety boundaries

This codec does **not** authenticate a writer, encrypt any data, guarantee atomic file writes, prevent torn pages, implement write-ahead ordering or define page allocation and reuse. CRC32 is accidental corruption detection only. WAL, doublewrite/full-page images, directory durability, filesystem-specific flush semantics, page generation counters and recovery from torn writes are future separately approved designs. In particular, the LSN header field has no durability significance until integrated with a WAL manager that enforces a write-ahead rule.

The intended integration contract is: a future buffer pool accepts only validated pages, pins versions across snapshots, and never exposes untrusted payload before structural and checksum verification. The WAL and checkpoint subsystems will define exactly when page bytes can be persisted and when an LSN can be acknowledged.

## Acceptance tests and later decisions

`cargo test -p bradian-relational` includes byte-level layout tests, every supported page type, maximum and empty payloads, corruption detection, unsupported-format rejection, and incorrect-length rejection. Cross-platform and power-loss qualification remain outstanding. Permanent page-size selection, encryption overhead, snapshot/generation semantics, multi-page records and row layout must be addressed before freezing the physical file format.
