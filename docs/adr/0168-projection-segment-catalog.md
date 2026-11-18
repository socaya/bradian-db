# ADR 0168 — Projection segment catalog (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Completes Batch E
by giving ADR 0163/0164 projection segments a
discoverable per-table index.

## 1. Scope

New module `projection_catalog`:

- `SegmentEntry { as_of_lsn, filename, bytes,
  registered_at_micros }`.
- `ProjectionIndex { segments: Vec<SegmentEntry> }`.
- `register_segment(dir, as_of_lsn, filename, bytes)`
  reads the current index, appends, writes atomically
  via `rename`. Duplicate LSN rejected.
- `load_index(dir)` — returns empty if no index file.
- `ProjectionIndex::segment_covering(target_lsn)`
  picks the latest segment at or below the target.
- Index serialized as `index.json` next to the
  `.brps` segments.

Decoupled from the main relational `Catalog` so OLAP
discovery state does not bloat OLTP metadata.

## 2. Out of scope

- **Garbage collection** of older segments.
- **Hierarchical / partitioned layout**.
- **Integration with main `Catalog`** — deliberate
  decoupling.
- **Index CRC** — segments carry their own CRC
  (ADR 0163).

## 3. Compliance

7 unit tests:
- empty directory loads as empty index.
- register-then-load round-trip.
- duplicate LSN rejected.
- `segment_covering` picks latest ≤ target.
- out-of-order inserts sorted by LSN in index.
- atomic rename leaves no `.tmp` artifact.
- entries carry registration timestamp.

## 4. Honest status

**Batch E (OLAP/HTAP)** reaches 100%:
- E1 committed projection (ADR 0134 + 0159 + 0163 + 0164).
- E2 watermark (ADR 0128).
- E3 vectorized reads (ADR 0133).
- **Catalog registration (this ADR)**.

BRD-HTAP-004 is now feature-complete end-to-end:
operators feed OLTP batches to the scheduler
(ADR 0164), which writes `.brps` segments and
registers them in `projection_catalog` for readers to
discover.
