# ADR 0119 — Catalog journal compactor (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch L L1 — BRD-REL-001
catalog-journal-growth primitive.

## Scope

- `CatalogSnapshot { version, payload: Catalog }`.
- `catalog_compactor::write_snapshot(path, &snapshot)`
  atomic (temp + rename, CRC32 trailer).
- `catalog_compactor::read_snapshot(path)` verifies CRC +
  magic + length.
- Catalog gains `#[derive(Serialize, Deserialize)]`.

## Out of scope

- **PersistentCatalog::open integration** — `open()` still
  replays the full journal. Taught to recognise the
  snapshot record in a follow-up ADR.
- **Online compaction** — operator takes the engine down
  before compacting.
- **Partial compaction** / incremental merges.

## Compliance

- Round-trip write + read preserves payload.
- Empty catalog round-trips.
- Missing magic → error.
- CRC tamper → error.
- 4 unit tests.
