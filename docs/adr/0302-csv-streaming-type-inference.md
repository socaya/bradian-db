# ADR 0302 — CSV streaming + type inference

## Status

Accepted. Second slice of P1-4.

## Context

`bradian_sources::csv::read_csv_bytes` already parses CSV
fully in memory and projects rows into `Change` envelopes.
P1-4's CSV slice calls for **streaming** (don't require the
whole file in memory), **type inference** (so downstream
callers can pick a column schema without hand-rolling one)
and **errors surfaced** (already present — a malformed
quote is rejected with context).

## Decision

Additive, inside `bradian_sources::csv`:

1. `read_csv_stream<R: Read>(reader: R, options)` returns a
   `CsvStream<R>` iterator yielding
   `BradianResult<CsvRow>` one row at a time. The header
   (when present) is consumed first and exposed via
   `CsvStream::header()`.
2. `infer_column_types(rows) -> Vec<InferredType>` with
   `InferredType::{Bool, I64, F64, Text}`. The rule: a
   column that is parseable as every non-empty row's value
   under the narrowest type wins; on conflict, widen. An
   all-empty column defaults to `Text`.
3. Both APIs sit beside `read_csv_bytes`; existing
   callers are untouched.

Scope limits:

- The streaming parser is a byte-at-a-time state machine
  with a small internal buffer. Not SIMD; not parallel.
- Type inference looks only at the first K rows the caller
  passes (caller-controlled); no automatic sampling. This
  slice keeps the function pure over its input.
- No automatic schema construction yet — that's a
  follow-up once a canonical column-name → `LogicalType`
  mapping is agreed.

## Consequences

- A CSV reader can now process a file larger than
  available memory, row by row.
- Type inference lets callers skip the boilerplate of
  classifying columns by regex.
- 3 unit tests: streaming parse matches `read_csv_bytes`
  on a 3-row input; type inference picks `I64 / F64 / Bool
  / Text` correctly; empty columns default to `Text`.
- No behaviour change to `read_csv_bytes`,
  `rows_to_changes` or the on-disk / envelope formats.
