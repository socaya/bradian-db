# ADR 0117 — Slow-query log ring buffer (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch F F3.

## Scope

- `SlowQueryLog::new(threshold, capacity)` — bounded ring
  buffer; records only statements whose elapsed exceeds
  `threshold`.
- `record(SlowQueryRecord)` — oldest evicted at capacity.
- `snapshot()` / `clear()` — operator-facing diagnostics.
- Shared handle semantics via `Arc<Mutex>`.

## Out of scope

- Durable storage — ADR 0104 audit log is the durable path.
- Automatic SQL-engine instrumentation — engines opt in by
  holding a log handle and calling `.record()`.
- Statement sampling / rate-limiting beyond ring capacity.

## Compliance

- Under-threshold records NOT retained.
- Over-threshold retained up to `capacity`; oldest
  evicted.
- Clone shares state (Arc-backed).
- 5 unit tests.
