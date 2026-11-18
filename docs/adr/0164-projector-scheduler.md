# ADR 0164 — Projector scheduler (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Final composition
primitive for BRD-HTAP-004. Wires:
- ADR 0128 `HtapWatermark` — the shared freshness state.
- ADR 0159 `projector_worker::drain_once` — the inner
  per-batch step.
- ADR 0163 `projection_segment::write_segment` —
  durable sink.

## 1. Scope

New module `projector_scheduler`:

- `ProjectorScheduler { watermark, target_dir,
  cancellation: Arc<AtomicBool> }`.
- `run_bounded(max_iterations, source_fn)` — loops
  up to `max_iterations` times. Each iteration calls
  `source_fn()` for the next batch set, then drains
  via `drain_once` writing a `.brps` segment per
  successful projection.
- Empty source short-circuits the loop.
- `cancellation_handle()` returns a shared
  `Arc<AtomicBool>` — flip from any thread to stop
  at the next iteration boundary (returns
  `SchedulerError::Cancelled`).
- `ScheduleReport { iterations_run,
  projections_written, last_segment }`.
- `SchedulerError { Projector | Segment | Cancelled }`.

Segment filenames encode the as_of_lsn:
`p-{LSN:020}.brps` so readers can discover projections
in LSN order with a filesystem scan.

## 2. Out of scope

- **Wall-clock interval** — synchronous; caller
  inserts `sleep` between iterations if needed. A
  tokio-driven variant is a follow-up.
- **Append-mode / resume semantics** — scheduler
  overwrites a segment with the same LSN if run
  twice. Caller supplies monotone LSNs.
- **Catalog registration of emitted segments** — a
  future ADR bridges segment discovery to the
  catalog's table metadata.
- **Backpressure negotiation** with downstream
  readers.
- **Multi-table scheduling** — one scheduler per
  watermark today.

## 3. Compliance

5 unit tests in `projector_scheduler::tests`:

- Scheduler writes a `.brps` segment per projection
  (verified via `read_segment`).
- Empty-source iteration short-circuits.
- Cancellation stops at the next iteration boundary.
- Watermark advances through successful batches.
- ScheduleReport counts projections + iterations.

## 4. Honest status

**BRD-HTAP-004 "committed columnar projection with
LSN witness"** is now production-complete at the
library layer:
- Watermark (ADR 0128).
- Row ↔ column converter (ADR 0133).
- Committed projection primitive (ADR 0134).
- Projector worker (ADR 0159).
- Durable segment writer (ADR 0163).
- Scheduler (ADR 0164).

Operators can wire any OLTP commit stream into the
scheduler and get LSN-tagged `.brps` segments on
disk. Catalog registration is the only remaining
cross-cutting integration.
