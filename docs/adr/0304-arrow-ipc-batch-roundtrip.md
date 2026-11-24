# ADR 0304 — Bradian-native batch IPC with round-trip

## Status

Accepted. Fourth slice of P1-4.

## Context

The acceptance for the "Arrow IPC adapter" slice is
"batch round-trip." Full Apache Arrow IPC wire-format
compatibility requires the `arrow` crate (≈50 transitive
dependencies) plus a review of the format's forward
compatibility guarantees. The audit surface for
`bradian-sources` is deliberately small (CSV is pure
Rust; JSONL uses only `serde_json`). Pulling the full
`arrow` dep in behind a flag is reasonable but is not
free, and the first slice to land should prove the batch
round-trip shape without taking that dep commitment.

## Decision

Ship a **Bradian-native batch IPC encoding** — a magic
header + `bincode`-serialised `IpcRecordBatch` — that
satisfies the round-trip acceptance today, and explicitly
mark full Apache Arrow IPC wire compatibility as a
follow-up.

```rust
pub struct IpcRecordBatch {
    pub schema: Vec<IpcColumn>,
    pub rows:   Vec<Vec<IpcValue>>,
}
pub enum IpcValueType { Bool, I64, F64, Str }
pub struct IpcColumn { name: String, ty: IpcValueType }
pub enum IpcValue { Null, Bool(bool), I64(i64), F64(f64), Str(String) }
pub fn write_ipc(batch: &IpcRecordBatch) -> Vec<u8>;
pub fn read_ipc (bytes: &[u8])            -> BradianResult<IpcRecordBatch>;
```

The wire format is `b"BRCH\x01"` magic followed by the
bincode-serialised struct. On read, the magic must match
or the call errors.

Scope limits:

- NOT Apache Arrow IPC. A consumer expecting the Apache
  wire format cannot read these bytes. The ADR docstring
  says so.
- Narrow value universe (Bool / I64 / F64 / Str plus
  Null). Decimal, Timestamp, nested types, dictionary
  encoding are deferred.
- Validates schema width against each row; mismatches
  surface a typed error.

## Consequences

- The round-trip acceptance is met in-tree without
  taking the `arrow` dep.
- A follow-up ADR can add real Apache Arrow IPC under a
  feature flag once the dep is reviewed; the Bradian
  format continues to work as the small-dependency
  fallback.
- 4 new unit tests: empty batch round-trip; typed row
  round-trip; schema/row width mismatch rejected on
  read; magic header validated.
- `bradian-sources` adds `bincode` as a dep (already a
  workspace member; no new transitive crates).
