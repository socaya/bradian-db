# ADR 0338 — Typed row-value encoding on the wire

## Status

Accepted. Replaces ADR 0316's debug-format row shape with
a typed, serde-stable `WireValue` enum. Closes the "typed
row-value encoding" follow-up from ADR 0316.

## Context

ADR 0316 shipped a working wire protocol that formatted
each row cell as `format!("{v:?}")`. That's fine for the
plaintext round-trip proof but useless for a real client:
`Int64(42)` and the string `"Int64(42)"` are
indistinguishable on the wire, and the `Debug` format is
not a stable contract.

A real wire protocol needs clients to parse cells back
into language-level types. The shape rustls / JWKS / every
other serde-tagged enum in this tree uses is a sibling
tag for the variant and a payload field.

## Decision

New `WireValue` enum mirroring
`bradian_relational::typed_table::Value`:

```rust
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireValue {
    Null,
    Bool { value: bool },
    Int32 { value: i32 },
    Int64 { value: i64 },
    Decimal { value: String },        // signed i128 as decimal string
    String { value: String },
    TimestampTz { micros: i64 },      // UTC μs since Unix epoch
    Uuid { value: String },           // canonical 8-4-4-4-12 dashed hex
    Json { value: String },           // canonical UTF-8 serde_json payload
}

impl From<bradian_relational::typed_table::Value> for WireValue { ... }
```

`WireMessage::RowSet.rows` changes from `Vec<Vec<String>>`
to `Vec<Vec<WireValue>>`. The handler converts each engine
`Value` through `WireValue::from(...)` before emission.

The memory-pressure estimator (ADR 0337) is updated to a
per-variant byte cost (`wire_value_bytes`) so it stays
proportional to the eventual serialised size.

## Scope

**In scope:**
- Every `typed_table::Value` variant lands on a typed
  wire variant with a stable JSON tag.
- UUID canonical dashed hex.
- Decimal as a decimal string (scale is NOT shipped in
  this slice — a column-type-hint header on `RowSet`
  is a follow-up once a receiver needs it).
- Typed memory-pressure byte estimation.
- 3 new tests on top of the 21 wire tests:
  1. Round-trip SELECT emits `WireValue::Int64 { value: 100 }`
     for a BIGINT column, not a `Debug`-string.
  2. `WireValue::from(Value::*)` is lossless-for-the-wire
     for every relational `Value` variant.
  3. Serde tag/field names are stable
     (`"type":"int64","value":42`) so a cross-language
     client can match on them.
- All 21 pre-existing tests still pass unchanged — the
  shape transformation is behaviour-preserving for
  everything downstream of `handle_connection`.

**Out of scope** (dedicated follow-ups):
- Per-column type-hint header on `RowSet` (so a client
  that wants the column's declared type without
  inferring from the first row has it).
- Binary framing (current wire is JSON-serialised
  values; a bincode / MessagePack path is a separate
  protocol choice).
- Decimal scale propagation.
- Streaming row emission.

## Consequences

- A client in any language can parse the row set back
  into typed values without string parsing.
- 24 wire tests total (3 new + 21 preserved). Workspace
  stays green under `cargo clippy --workspace
  --all-targets --locked -- -D warnings`.
- Breaking change to the on-the-wire row format; there
  are no external clients yet so the migration cost is
  zero today. The `Hello.version: u32` field is the
  slot a future bump would live in.
