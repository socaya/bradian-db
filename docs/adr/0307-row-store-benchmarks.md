# ADR 0307 — Row-store write / read benchmark published

## Status

Accepted. Second slice of P1-5, closing P1-5 and all of P1.

## Context

The P1-5 benchmarks acceptance explicitly says "publishes
numbers in `docs/`". Prior to this slice `bradian-benchmark`
had a Criterion harness scaffolded (4 benches) but no
dedicated RowStore bench and no in-repo reference numbers
for the Gate-1 WAL-backed row store.

## Decision

1. Add `crates/bradian-benchmark/benches/row_store_rw.rs`
   with two Criterion benches:
   - `row_store_put_fresh`      — single-op WAL-fsynced
     `put` latency on a fresh store (each sample opens a
     new store + writes one row).
   - `row_store_get_checkpointed` — single-op hot `get`
     latency against a 10 000-entry pre-seeded + checkpointed
     store (warm cache).

2. Run the bench once on the developer workstation with
   `--sample-size 20 --measurement-time 5` and publish the
   numbers + full environment context at
   [`docs/bench/row_store_rw-2026-11-24.md`](../bench/row_store_rw-2026-11-24.md).

Scope limits:

- Single-op latency only. Bulk throughput is deliberately
  out of scope; the bench file documents the extension
  path for future slices.
- Laptop-class numbers. The report explicitly says the
  numbers are not production-regulated and cites spec §12
  for the full `BenchContext` requirement.
- Numbers represent commit `f1c7c2d` (post-0306, pre-0307).

## Consequences

- Reference wall-time numbers are now in-repo and versioned
  with the code.
- Future regressions on the two measured paths can be
  chased against these numbers; a subsequent-date bench
  file at the same shape is the drop-in format.
- P1-5 is now 2 of 2 DONE. **All P1 slices closed.**
