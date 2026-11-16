# ADR 0112 — SQL subset corpus extension (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Partial close of
G1-REFERENCE "documented isolation qualification and external
reference-suite" via incremental corpus extension.

## 1. Scope

9 new corpus entries (every claim backed by a passing test
in `sql_subset_corpus.rs`):

**Rejected grammar** — new §5 rows:
1. `join_syntax_in_select_is_rejected` — JOIN / INNER JOIN.
2. `subquery_in_where_is_rejected` — subquery in WHERE.
3. `select_without_from_is_rejected` — SELECT 1.
4. `insert_select_is_rejected` — INSERT ... SELECT.
5. `insert_values_with_wrong_column_count_is_rejected` —
   column-count mismatch.
6. `update_from_syntax_is_rejected` — UPDATE ... FROM.

**Honest boundaries**:
7. `json_data_type_in_create_table_is_rejected_by_parser` —
   JSON (ADR 0099) exists at core level but unreachable via
   SQL DDL (matches TimestampTz/Uuid boundary).
8. `integer_literal_overflow_i32_coerces_or_errors_but_never_panics`
   — i32 overflow on INT column is a clean error.
9. `duplicate_primary_key_on_insert_is_rejected` — duplicate
   PK is rejected and the original row is preserved.
10. `both_keywords_and_identifiers_are_case_insensitive` —
    **honest observation**: pre-existing engine behaviour
    was that identifiers are case-insensitive (lower-cased
    at parse time). Recorded rather than changed.

## 2. Out of scope

- **External SQL-92/99/2016 reference-suite compliance** —
  still OPEN under G1-REFERENCE.
- **Quoted identifiers** (`"MyTable"`) — would enable case-
  sensitive identifiers.
- **JSON SQL path** — `LogicalType::Json` requires a DDL
  grammar addition plus `DataType::Json` in the model; out
  of scope for this slice.
- **Prepared-statement SQL syntax** — ADR 0100 added the
  Rust API; `?` placeholders are not yet documented in the
  corpus because they're a client-side substitution layer,
  not a parser feature.

## 3. Compliance

1. 9 new tests pass alongside the pre-existing 37.
2. Corpus doc §1 updated to list JSON alongside TimestampTz/
   Uuid as SQL-unreachable.
3. §5 "Rejected grammar" extended with 8 new rows.
4. §5 now explicitly records identifier case-insensitivity.
5. Total corpus: 46 tests in `sql_subset_corpus.rs`.
