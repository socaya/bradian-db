# Master Gate 1 — `Value::Json` variant verification

21 November 2026. ADR 0173. BRD-REL-004 feature-shortfall
closure; **no persistent format break** (new tag 8 added;
legacy tag 5 rows still decode under a JSON column).

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test json_value_type` | 7 passed, 0 failed |
| `cargo test --locked -p bradian-relational --test sql_subset_corpus json` | 1 passed (`json_data_type_in_create_table_is_accepted_after_adr_0113`) |
| `cargo check -p bradian-relational --all-targets --locked` | Passed |

## Finding and action

ADR 0113 made `DataType::Json` reachable through the SQL
parser but stored and returned the coerced value as
`Value::String`, so a SELECT of a JSON column lost the
type distinction. BRD-REL-004 "JSON type" (Priority 2
feature shortfall per
`BRADIAN_COMPREHENSIVE_GAPS_ANALYSIS.md` §10 item 11) was
listed as not fully implemented for this reason.

Change (ADR 0173):

- Added `typed_table::Value::Json(String)` variant.
- Added encoding tag `8` in `write_value` with the same
  length-prefixed UTF-8 body shape as String tag `5`.
- `read_value` decodes tag `8` back to `Value::Json`.
- Backward compatibility: legacy tag-5 rows under a JSON
  column still decode (as `Value::String`) and validate
  through `validate_value`, which accepts both pairings
  for `DataType::Json` as long as the content parses
  with `serde_json`.
- `coerce` in `sql.rs` now emits `Value::Json(s)` for a
  JSON column (previously `Value::String(s)`).
- Exhaustive-match updates in `query_budget::row_bytes`,
  `select_window::matches_type`, `select_window::compare`,
  `driver::value_to_sql`,
  `sql::value_to_sql_literal` and
  `typed_table::decoded_row_bytes`. In every site Json
  folds into the String branch (same byte-count,
  lexicographic order, quoted-literal rendering).
- `BRADIAN_SQL_SUBSET.md` §1 updated: JSON, TIMESTAMPTZ,
  UUID now have explicit observed-variant rows with
  covering test names. §5 rejection rows for those
  types removed; the "unsupported type keywords" row
  continues to cite
  `unsupported_data_types_in_create_table_are_rejected`
  which ADR 0113 updated in place to test BLOB/FLOAT.

New test file
`crates/bradian-relational/tests/json_value_type.rs`
(7 tests):

1. `encode_decode_round_trip_preserves_json_variant` —
   bytes[24] == tag 8.
2. `legacy_tag_5_string_rows_still_decode_for_json_columns`
   — hand-crafted legacy row decodes as `Value::String`
   and validates.
3. `invalid_json_body_is_rejected_at_encode`.
4. `sql_engine_insert_round_trip_returns_value_json` —
   full SQL INSERT + SELECT path.
5. `invalid_json_literal_rejected_at_insert_through_sql`.
6. `null_insert_into_nullable_json_column_round_trips`.
7. `update_json_column_preserves_value_json_tag`.

## Scope limits (honest)

- **No JSON path operators** (`->`, `->>`, `@>`). Parser
  expansion is deferred to a follow-up ADR (Gate 2
  territory).
- **No JSONB / binary JSON**. Canonical UTF-8 only.
- **No full-table re-encoding of pre-ADR-0173 rows**.
  Legacy tag-5 rows continue to read back as
  `Value::String`; new rows get tag 8. A future ADR
  could add an online re-encode if downstream tooling
  needs the storage-level type distinction.
- **No COLLATE semantics on JSON columns**. Lexicographic
  byte order only.

## Evidence

- Source:
  `crates/bradian-relational/src/typed_table.rs` (new
  variant, new tag 8, backward-compat decoder, dual-
  pairing `validate_value`),
  `crates/bradian-relational/src/sql.rs` (`coerce`
  emits `Value::Json`; `value_to_sql_literal` fold),
  `crates/bradian-relational/src/select_window.rs`
  (`matches_type` + `compare` updated),
  `crates/bradian-relational/src/query_budget.rs`
  (`row_bytes` fold),
  `crates/bradian-relational/src/driver.rs`
  (`value_to_sql` fold).
- Tests:
  `crates/bradian-relational/tests/json_value_type.rs`
  (7 new tests);
  `crates/bradian-relational/tests/sql_subset_corpus.rs`
  `json_data_type_in_create_table_is_accepted_after_adr_0113`
  remains green.
- ADR: `docs/adr/0173-value-json-variant.md`.
- Reference doc: `docs/reference/BRADIAN_SQL_SUBSET.md`
  §1, §5 updated.
