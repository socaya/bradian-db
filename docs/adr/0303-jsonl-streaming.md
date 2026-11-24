# ADR 0303 — JSON Lines streaming adapter

## Status

Accepted. Third slice of P1-4.

## Context

`bradian_sources::jsonl::read_jsonl` already parses a full
JSONL blob in memory. P1-4 asks for the same streaming
treatment the CSV adapter just received (ADR 0302): read a
large JSONL source row-by-row from any `std::io::Read`.

## Decision

Additive, inside `bradian_sources::jsonl`:

1. `JsonlStream<R: Read>` is an iterator yielding
   `BradianResult<serde_json::Value>` one line at a time.
   Blank lines are skipped. Parse errors carry the
   1-based line number so a caller can localise a
   malformed record.
2. `JsonlEnvelopeStream<R: Read>` wraps `JsonlStream` and
   parses each record directly into a `Change` envelope
   (the streaming equivalent of `read_jsonl` with
   `expect_envelope=true`).

Scope limits:

- One record per line. Multi-line / pretty-printed JSON is
  out of scope — the format is "JSON Lines" exactly.
- No schema inference analogous to CSV's
  `infer_column_types`; JSON values carry their own types.

## Consequences

- JSONL sources can be processed row-by-row with bounded
  memory; the ingester can drive directly off the stream
  without first materialising a `Vec<Change>`.
- 3 new unit tests: streaming parse parity against the
  batch parser; per-line error reporting carries the
  offending line number; blank lines are skipped.
- `read_jsonl` and `read_jsonl_with` stay as-is.
