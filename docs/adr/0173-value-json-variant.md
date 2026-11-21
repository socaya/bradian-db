# ADR 0173 — `Value::Json` variant for round-trip JSON preservation (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the
code-writable portion of BRD-REL-004 "JSON type".

## 1. Scope

ADR 0113 made `DataType::Json` reachable through the SQL
parser and `coerce`, but the coerced cell was stored and
returned as `Value::String` — SQL round-trip of a `JSON`
column lost the declared type distinction. This ADR adds a
distinct `Value::Json(String)` variant so `SELECT` of a
`JSON` column returns `Value::Json`, while every stored row
remains a serde_json-valid canonical UTF-8 payload.

Change surface:

- **New variant**: `typed_table::Value::Json(String)`.
- **New encoding tag**: `8` in `write_value`, with the same
  `u32 length + utf8 bytes` body shape as the String tag.
- **Decoder**: tag `8` reads back as `Value::Json`.
  Legacy rows written under tag `5` (String) for a column
  declared `JSON` still decode (as `Value::String`) and
  validate — no persistent-format break.
- **`validate_value`**: accepts both
  `(DataType::Json, Value::Json(s))` and
  `(DataType::Json, Value::String(s))` provided
  `serde_json::from_str::<serde_json::Value>(s)` succeeds.
- **`coerce`** in `sql.rs`: emits `Value::Json(s)` for a
  JSON column (previously `Value::String(s)`).
- **`select_window::compare`**: Json compares as its
  canonical UTF-8 bytes; legacy String-encoded JSON
  interleaves naturally with new Json-tagged rows for the
  same declared column type.
- **`select_window::matches_type`**: accepts
  `(Value::Json, DataType::Json)` and the legacy
  `(Value::String, DataType::Json)` pairing for
  pre-ADR-0173 rows.
- **`query_budget::row_bytes`,
  `driver::value_to_sql`,
  `sql::value_to_sql_literal`,
  `typed_table::decoded_row_bytes`**: fold Json into their
  String branch (same len / escaped-quote rendering).

## 2. Out of scope

- **JSON path operators (`->`, `->>`, `@>`)**: ADR 0113
  §2 keeps them as a follow-up. Still not implemented.
- **`DataType::Json` COLLATE semantics**: Json columns
  intentionally reject the COLLATE clause (lexicographic
  byte order only).
- **Backfill of pre-ADR-0173 stored rows**: legacy
  tag-5 rows continue to read back as `Value::String`.
  A future full-table rewrite ADR could re-encode them
  under tag 8 if downstream tooling needs the type
  distinction on stored bytes; today only new rows get
  tag 8.
- **Binary JSON (JSONB)**: canonical UTF-8 only; no
  binary representation.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/json_value_type.rs`
(7 tests, all green):

- `encode_decode_round_trip_preserves_json_variant` —
  `encode_row` writes tag `8` at the JSON column offset;
  `decode_row` round-trips to the same `Value::Json`.
- `legacy_tag_5_string_rows_still_decode_for_json_columns`
  — hand-crafted legacy row with tag `5` under a JSON
  column decodes as `Value::String` and `validate_value`
  accepts it.
- `invalid_json_body_is_rejected_at_encode` — a
  `Value::Json("not json {")` row is rejected by
  `encode_row` via `validate_value`.
- `sql_engine_insert_round_trip_returns_value_json` —
  INSERT via SQL + SELECT returns `Value::Json(s)`.
- `invalid_json_literal_rejected_at_insert_through_sql` —
  an invalid JSON literal is rejected at `coerce`.
- `null_insert_into_nullable_json_column_round_trips` —
  `NULL` into a nullable `JSON` column returns
  `Value::Null`.
- `update_json_column_preserves_value_json_tag` — UPDATE
  of a JSON column returns `Value::Json` after re-read.

Existing ADR 0113 test
`json_data_type_in_create_table_is_accepted_after_adr_0113`
continues to pass; the parser, validator and storage
protocol stay compatible.

## 4. BRADIAN_SQL_SUBSET.md impact

- Section 1 — `JSON`, `TIMESTAMPTZ`, `UUID` rows added to
  the "Data types" table with their observed `Value`
  variants and covering test names.
- Section 5 — the two rejection rows for
  `TIMESTAMPTZ` / `UUID` and `JSON` in `CREATE TABLE`
  removed. The remaining "unsupported type keywords"
  rejection row cites the same
  `unsupported_data_types_in_create_table_are_rejected`
  test (which ADR 0113 updated in place to test BLOB/
  FLOAT).

## 5. Roadmap impact

BRD-REL-004 "JSON type" (Priority 2 feature shortfall
per `BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md` §10) closes
its code-writable scope. Remaining BRD-REL-004 work is
`TimestampTz` / `Uuid` round-trip variant parity — both
already round-trip as their declared variant per ADR
0113, no further change needed — and JSON path operators,
which require parser expansion (Gate 2 territory).
