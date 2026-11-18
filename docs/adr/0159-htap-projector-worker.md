# ADR 0159 — HTAP projector worker (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Promotes
BRD-HTAP-004 "committed columnar projection with LSN
witness" from FOUNDATIONS to PARTIAL: ADR 0128 shipped
`HtapWatermark`, ADR 0133 shipped the row ↔ column
converter, ADR 0134 shipped `CommittedProjection`. This
ADR wires the three into a single-threaded worker that
advances the OLAP side of the watermark.

## 1. Scope

New module `crates/bradian-relational/src/projector_worker.rs`:

- `SourceBatch { as_of_lsn, batch: RowBatch }` — the
  caller's source yields these.
- `ProjectorError` wraps the three composed error
  types: `Projection(ProjectionError)`,
  `Batch(BatchError)`, `Sink { lsn, reason }`.
- `drain_once(watermark, source, sink) -> Result<u64>`
  — iterator-driven worker. For each `SourceBatch`:
  1. Convert rows → columns via `rows_to_columns`.
  2. Wrap in a `CommittedProjection` via
     `project_committed` (validates
     `as_of_lsn <= watermark.latest_committed()`).
  3. Hand the projection to `sink(projection)`; sink
     returns `Result<(), String>`.
  4. On success, `watermark.advance_projected(lsn)`.
  5. On any error, stop and return it. Prior
     successful batches have already advanced.

The worker is single-threaded and bounded by the
iterator the caller supplies. Pure plumbing.

## 2. Out of scope (follow-ups)

- **Durable segment writer** — the sink callback
  receives `CommittedProjection` in-memory. Writing
  a `.brcs` segment per ADR 0082 is a sink
  implementation belonging in a future ADR.
- **Multi-table / per-partition scheduling** — one
  watermark + one source per worker today. Scheduler
  deferred.
- **Catch-up policy** — caller decides when to
  invoke `drain_once` vs. tight loop. A future ADR
  can ship a cron / tokio-interval driver.
- **Backpressure on the sink** — sink's `Err` stops
  the worker; it cannot pause-and-resume mid-batch.
- **Catalog awareness** — the worker has no catalog;
  the caller feeds it `RowBatch`es. A future ADR can
  bridge from `Table` metadata to `SourceBatch`
  generation.

## 3. Compliance

6 unit tests in `projector_worker::tests`:

- `drain_advances_projected_watermark_for_every_batch`
  — 3 batches → watermark at last LSN.
- `drain_stops_at_first_sink_error_but_keeps_prior_progress`
  — second-batch sink error stops iteration; prior
  advance is preserved.
- `drain_refuses_future_lsn_above_committed` —
  `Projection::FutureProjection` surfaces through the
  worker.
- `drain_empty_source_is_a_no_op`.
- `batch_width_mismatch_surfaces_as_batch_error` —
  `BatchError` surfaces through the worker.
- `projection_carries_the_source_lsn` — sink
  observes the LSN witness.

## 4. Honest status

**BRD-HTAP-004** moves from FOUNDATIONS to PARTIAL:
the watermark advance loop is wired across every
primitive. Durable segment writer, scheduler,
backpressure protocol, and catalog bridge remain
OPEN per §2.
