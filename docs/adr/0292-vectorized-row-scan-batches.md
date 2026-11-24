# ADR 0292 — Vectorized SELECT path: RowStore rows → RecordBatches

## Status

Accepted. First slice of P1-1 (columnar / analytical engine
wiring).

## Context

`bradian-runtime` already exposes a vectorized batch
executor (`RecordBatch`, `RecordBatchBuilder`,
`parallel_filter_aggregate`, scalar predicate oracle). It
reads from `.brcs` segments and in-memory partitions, but
not from the relational WAL-backed row store. Meanwhile,
`bradian-relational::TableStore` stores typed rows backed by
`RowStore`, and SQL execution returns `Vec<Vec<Value>>` row
batches.

The two sides have independent `Value` types
(`bradian_core::Value` vs `bradian_relational::typed_table::Value`)
and independent type systems
(`LogicalType` vs `DataType`), so wiring is not implicit —
a concrete adapter is required.

G1 code is complete. P1-1 can now start. ADR 0292 is the
first slice: a vectorized SELECT path that consumes rows
read from a real `TableStore` (RowStore-backed).

## Decision

1. Add `bradian-runtime` as a `dev-dependency` of
   `bradian-relational` so the parity path can be tested
   without making the runtime a hard dep of the relational
   crate yet. Promotion to a production dep belongs to
   ADR 0293 (columnar segment write path from commit).
2. Ship a new integration test
   `crates/bradian-relational/tests/vectorized_scan_parity.rs`
   that:
   - Opens a real `TableStore` on a tempdir; inserts N rows
     with mixed types (`Int64` PK, `Int64` score, `Text`
     category).
   - Scans typed rows via `TableStore::scan()` — these are
     rows that physically came out of the RowStore WAL.
   - Maps relational `DataType` → core `LogicalType` and
     relational `Value` → core `Value` via a local adapter
     explicitly scoped to the shared-type subset (Int64,
     Text; Null preserved).
   - Chunks mapped rows into batches of a bounded size
     (`batch_size = 4`) using
     `bradian_runtime::RecordBatchBuilder`.
   - Executes the vectorized path:
     `parallel_filter_aggregate` with `Predicate::Compare`
     on `score > 50` and `AggregateKind::CountStar`.
   - Baseline: scalar row-by-row filter over the same rows,
     counting matches directly.
   - Asserts the batch COUNT equals the scalar COUNT and
     the row-materialized batch contents reproduce the
     original rows one-for-one.
3. Honest scope limits captured in test comments:
   - Only the shared-type subset is covered. UUID, Decimal,
     TimestampTz, Json cross-type mapping is explicitly
     deferred to follow-up slices under ADR 0293/0294.
   - The adapter remains a test helper; no public
     crate-level API is exported for the mapping yet.
   - COMMIT→batch pipelining and incremental view
     maintenance remain OPEN (ADR 0293/0294).

## Consequences

- G1 code remains unchanged; no production code in
  `bradian-relational` or `bradian-runtime` moved.
- The P1-1 wiring now has a demonstrable parity test
  showing the batch executor correctly consumes RowStore
  rows under the shared-type subset.
- Future slices can lift the type-mapping into a public
  module (and widen the type coverage) without rewriting
  the test shape.
- 1 integration test (`vectorized_row_scan_parity`) covers
  the batch/row-path parity contract.
