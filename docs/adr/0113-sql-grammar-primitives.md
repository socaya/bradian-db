# ADR 0113 — SQL grammar primitives: JSON / TimestampTz / Uuid / TRUNCATE / profile string (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch A software-only
gaps A9–A15 subset. Advanced grammar items (JOIN / aggregates
/ range / OR / aliases / DISTINCT / CHECK / FK actions /
quoted idents / COLLATE wiring / prepared named params) are
larger parser rewrites deferred to follow-up ADRs.

## 1. Scope

- **New DataType variant**: `DataType::Json` with
  `#[serde(default)]`-tolerant Table extension
  `column_collations` map (also new, same pattern).
- **SQL parser accepts**: `JSON`, `TIMESTAMPTZ` / `TIMESTAMP`,
  `UUID` in `CREATE TABLE` column specs.
- **coerce path**:
  - JSON: `TEXT` literal validated via `serde_json`; stored
    as `Value::String`.
  - TimestampTz: integer literal interpreted as micros since
    epoch; stored as `Value::TimestampTz(i64)`.
  - Uuid: 32-hex-char literal (dashes optional); stored as
    `Value::Uuid([u8; 16])`.
- **typed-table validation**: `DataType::Json × Value::String`
  accepts iff `serde_json::from_str` succeeds.
- **TRUNCATE TABLE**: new statement that scans PKs and
  deletes each row; preserves schema.
- **Versioned SQL profile**: `pub const SQL_PROFILE_VERSION:
  &str = "bradian-sql-g1-2026-11-16"`. Clients can detect
  profile-compat without parsing.
- **Corpus doc**: ADR 0099 / 0112 "JSON unreachable" boundary
  reversed; new `json_data_type_in_create_table_is_accepted_
  after_adr_0113` test replaces it.

## 2. Out of scope (next ADRs)

- **COLLATE clause parsing**: `column_collations` map exists
  on Table but CREATE TABLE doesn't accept `COLLATE <name>`
  syntax yet.
- **Quoted identifiers** (`"MyTable"`) — follow-up.
- **JSON path operators** (`->`, `->>`) — follow-up.
- **ISO-string timestamp literals** — current accepts only
  integer micros.
- **FOREIGN KEY action keywords** (`CASCADE`, `SET NULL`,
  `RESTRICT`) — parser still refuses them.
- **CHECK constraint parsing** — follow-up.
- **Aggregates / JOIN / range predicates / OR / column
  aliases / DISTINCT / subquery / INSERT SELECT / UPDATE
  FROM** — parser still refuses per ADR 0112 §5. These need
  a substantial parser rewrite (Gate 2 territory).

## 3. Compliance

1. `CREATE TABLE ... JSON NOT NULL` accepted; valid JSON
   insert round-trips.
2. Invalid JSON literal rejected with `Type` / `Table` error.
3. `UUID` column accepts 32-hex (with or without dashes);
   non-hex rejected.
4. `TIMESTAMPTZ` column accepts integer micros literal;
   round-trips as `Value::TimestampTz`.
5. `TRUNCATE TABLE t.public.items` deletes every row;
   schema preserved; subsequent INSERT succeeds.
6. `TRUNCATE` on empty table reports `affected: 0`.
7. `SQL_PROFILE_VERSION` is a stable `const &str`.
8. 7 new tests in `sql_grammar_extensions.rs`.
9. Corpus doc §1 + §5 updated; ADR 0112 stale tests fixed.

1237 workspace tests pass (1230 → 1237).
