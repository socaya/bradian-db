# ADR 0096 — Mutable page-file allocator foundation (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-15 as a Gate-1-scope foundation.**

## 1. Honest scope

This ADR delivers a **primitive** — a crash-recoverable mutable
page-file with append-only free-list manifest — that a future
crash-safe B+tree (BRD-STO-005, Gate 3) can sit on. It is
**not** a B+tree, not a buffer pool, not a transaction manager,
and not production mutable-storage.

What ships:
- `MutablePageFile`: fixed 4 KiB pages identified by a stable
  `page_id: u64`. Each page carries a 16-byte header
  (`BRMP\x01\x00` magic, generation u16, kind u8, reserved u8,
  u32 len, u32 crc32).
- Allocation via a free-list manifest written out-of-band to a
  `freelist.manifest` file after each page allocation.
- Crash recovery: on reopen, every page CRC is verified, the
  manifest is replayed, and in-flight allocations are reclaimed.
- `write_page`/`read_page`/`free_page` API.
- Tests for CRC detection, free-list replay, torn-write recovery,
  and parallel-open rejection via the same `rows.lock` convention
  as `RowStore`.

What remains open (explicit non-goals per CLAUDE.md §2):
- **Full B+tree on mutable pages** (BRD-STO-005, Gate 3).
- **Buffer pool / page cache** with eviction (BRD-STO-005).
- **Concurrent readers during writer page swaps**.
- **Multi-version page chains** for MVCC on mutable pages.
- **Log-structured merge trees, LRU, allocator fairness**.

## 2. Compliance

1. Writer: `write_page(page_id, kind, payload) -> ()` CRCs the
   full header+payload before writing, fsyncs, appends an
   allocation entry to `freelist.manifest`.
2. Reader: `read_page(page_id) -> Vec<u8>` verifies CRC + magic
   + generation; CRC mismatch returns `Corruption`.
3. Reopen: torn page writes (short tail, bad CRC) are detected
   and reported; the file length is preserved.
4. `rows.lock` convention: second open on the same directory
   returns `Busy`.
5. `free_page(id)` appends a tombstone; next `allocate()` reuses
   the slot.
6. Tests cover: fresh write + read, CRC tamper detection, torn
   write (header-only, body-partial), parallel-open race,
   free-list replay across restart.
7. Clippy clean.

## 3. Out of scope

- All BRD-STO-005 invariants beyond "pages persist + survive
  reboot". Page eviction, buffer pool, write-ahead logging on
  the mutable file, page-level MVCC, and B+tree operations
  remain Gate-3 work.
