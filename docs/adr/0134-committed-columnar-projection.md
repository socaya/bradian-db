# ADR 0134 — Committed columnar projection primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch E E1 primitive —
completes Batch E. Composes ADR 0128 (HTAP freshness
watermark) with ADR 0133 (row↔column batch converter).

## 1. Scope

New module `bradian_relational::committed_projection`:

- `CommittedProjection { batch: Vec<ColumnBatch>,
  as_of_lsn: u64 }` — the LSN witness travels with the
  data.
- `project_committed(batch, as_of_lsn, watermark)` wraps
  a batch iff `as_of_lsn <= watermark.latest_committed()`.
- `CommittedProjection::is_fresh_as_of(reader_lsn)` —
  true iff `as_of_lsn >= reader_lsn`.
- `width()` / `height()` accessors.
- `ProjectionError { FutureProjection / EmptyBatch }`.

## 2. Out of scope (follow-ups)

- **Durable segment-file writer / reader** for
  projections — ADR 0082 covers DictIndex32; a future
  ADR can emit `.brcs` segments stamped with this
  LSN witness.
- **Automatic projection catch-up worker** — the
  watermark's projected side is still caller-driven.
- **Multi-table / per-partition projections** — one
  flat `Vec<ColumnBatch>` per instance.
- **Zero-copy projection sharing** — today the
  primitive clones. A future `Arc`-wrapped variant
  can share pages.
- **Schema reconciliation across projections** — if a
  column is DROPped (ADR 0124) or RENAMEd between
  projections, the caller must invalidate stale
  projections.

## 3. Compliance

- Projection with `as_of_lsn <= watermark.committed`
  accepted.
- Projection exactly at watermark accepted.
- Projection above watermark rejected with
  `FutureProjection { as_of_lsn, watermark }`.
- Empty batch rejected with `EmptyBatch`.
- `is_fresh_as_of(lsn)` matches the inequality.
- `(0, 0)` watermark + LSN is valid.
- `width()` / `height()` report correctly.
- Clone + PartialEq round-trip identity.
- 8 new tests in `committed_projection::tests`.

## 4. Honest status

Foundation only. **BRD-HTAP-004 committed columnar
projection with LSN witness** is not yet satisfied
because no engine emits projections, no segment
writer persists them, and no catch-up worker advances
the OLAP watermark automatically. The primitive is the
prerequisite that a future columnar engine consumes.
