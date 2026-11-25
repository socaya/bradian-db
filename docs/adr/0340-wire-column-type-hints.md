# ADR 0340 — Per-column `WireType` hints on `RowSet`

## Status

Accepted. Extends ADR 0338 with a `column_types` list on
`WireMessage::RowSet` so clients know each column's
expected variant before parsing the first row — critical
for empty-row-set responses where variant inference from
row data is impossible.

## Context

ADR 0338 made row cells typed (`WireValue` with a serde
tag). A client receiving `RowSet { columns: ["age"],
rows: [] }` still had no idea what cell variant to expect
on the next request. A production wire protocol needs
the column-type metadata in the row-set header.

The engine doesn't yet surface declared column types on
`SqlResultSet::Rows`, so this slice ships a best-effort
inferred version with a documented follow-up for the
engine-sourced path.

## Decision

New `WireType` enum (one entry per `WireValue` variant
plus `Unknown` for the "can't tell" case). Add a
`column_types: Vec<WireType>` field to
`WireMessage::RowSet`.

```rust
#[serde(rename_all = "snake_case")]
pub enum WireType {
    Unknown, Bool, Int32, Int64, Decimal,
    String,  TimestampTz, Uuid, Json,
}

pub enum WireMessage {
    ...
    RowSet {
        columns:      Vec<String>,
        column_types: Vec<WireType>,
        rows:         Vec<Vec<WireValue>>,
    },
    ...
}
```

Semantics:

- `WireValue::wire_type(&self) -> WireType` is the
  discriminant. `Null` maps to `Unknown` because a lone
  null tells a client nothing about the declared type.
- `infer_column_types(columns, rows)` walks the first
  non-null cell per column. Columns that remain all-null
  (or empty row set) stay `Unknown`. Clients SHOULD
  treat `Unknown` as "accept any variant".
- Serde tags are `snake_case` string literals
  (`"timestamp_tz"`, etc.) so a non-Rust client parses
  them with no additional context.

## Scope

**In scope:**
- `WireType` enum + per-value discriminant.
- First-non-null inference.
- 6 new tests (36 total in wire):
  1. End-to-end SELECT emits `column_types == [Int64]`
     for a BIGINT column.
  2. `WireValue::wire_type()` stable per variant.
  3. Inference skips nulls and picks the first typed
     cell per column.
  4. All-null column stays `Unknown`.
  5. Empty row set → all-`Unknown` column types.
  6. `WireType` serialises as `snake_case` (e.g.
     `"timestamp_tz"`).

**Out of scope** (dedicated follow-ups):
- Engine-sourced column types from
  `bradian_relational::SqlResultSet`. Currently
  `SqlResultSet::Rows` carries only column *names*; a
  one-slice follow-up adds `column_types: Vec<DataType>`
  to the engine's own result set and threads it through
  the wire handler, eliminating the inference step.
- Decimal-scale metadata — engine-sourced types will
  carry scale directly.
- `nullable` flag — currently inferred as "anything can
  be null"; an engine-sourced flag is cleaner.

## Consequences

- A client receiving an empty `RowSet` can still render
  a typed header row (column name + `WireType`).
- Breaking shape change on `RowSet` — no external
  clients today so migration cost is zero. The
  `Hello.version: u32` field is the slot a future bump
  would live in.
- 36 wire tests total. Workspace stays green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`.
