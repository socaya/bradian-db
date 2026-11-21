# ADR 0175 — `COLLATE` clause in `CREATE TABLE` + ORDER BY usage (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-21.** Closes the ADR 0113 §2
and ADR 0103 §2 follow-up:

> **COLLATE clause parsing**: `column_collations` map exists
> on Table but CREATE TABLE doesn't accept `COLLATE <name>`
> syntax yet. — ADR 0113 §2
>
> **Catalog storage / DDL syntax** — no `COLLATE` clause in
> CREATE TABLE yet. Operators use this primitive directly
> in-code. — ADR 0103 §2

## 1. Scope

- Parser: accept an optional `COLLATE <name>` clause after
  the nullability modifier in each CREATE TABLE column.
  Accepted names: `binary` (default) and `nocase`
  (case-insensitive ASCII).
- `TableSpec` gains a `column_collations:
  BTreeMap<String, Collation>` field. `#[serde(default,
  skip_serializing_if = "BTreeMap::is_empty")]` preserves
  pre-ADR-0175 spec bytes.
- Catalog `create_table` validates each key:
  - Must name an existing column
    (`CatalogError::InvalidDefinition`).
  - Column type must be `Text` or `Varchar { .. }`; any
    other type is a definition error.
- `select_window::SortKey` gains a `collation` field
  populated from the table's `column_collations` at
  parse time; non-text columns always resolve to
  `Binary`.
- `select_window::compare` applies the collation to
  `Value::String` / `Value::Json` pairs during ORDER BY
  sorting.
- Primary-key tie-break ignores COLLATE: logical PK
  ordering remains binary so ties stay deterministic.
- `PrimaryKey`, `TableSpec` derive `Default` to support
  `..Default::default()` on construction.
- Unknown `COLLATE <name>` → `SqlError::Unsupported`.
- `COLLATE` on a non-text column → `SqlError::Catalog`
  (the catalog's `InvalidDefinition`).
- `BRADIAN_SQL_SUBSET.md` updated to document the
  clause and show the two accepted names.

## 2. Out of scope

- **Full Unicode UCA / locale-aware collation.** `nocase`
  is ASCII case folding only; non-ASCII compares
  bytewise (honest boundary documented in ADR 0103).
  ICU-based collations deferred.
- **COLLATE in WHERE equality / UNIQUE constraints.**
  Equality still uses byte semantics; a `nocase` TEXT
  column's PK/UNIQUE enforcement remains byte-exact.
  Rationale: changing PK/UNIQUE comparison semantics
  requires an index rebuild and interacts with ADR 0127
  secondary-index helpers; a follow-up ADR can wire it.
- **COLLATE on JSON columns.** JSON always compares as
  its canonical UTF-8 bytes; the parser rejects COLLATE
  on non-text columns.
- **Dynamic / per-query `ORDER BY ... COLLATE <name>`.**
  Collation is declared at CREATE TABLE and used
  implicitly. Per-query override is a larger grammar
  change deferred to a follow-up.

## 3. Compliance

New integration test file
`crates/bradian-relational/tests/collation_sql.rs`
(6 tests, all green):

1. `create_table_accepts_collate_binary_and_nocase_on_text_columns`
2. `create_table_rejects_unknown_collate_name` — `icu_root`
   name → `SqlError::Unsupported`.
3. `create_table_rejects_collate_on_non_text_column` —
   COLLATE on BIGINT → `SqlError::Catalog`.
4. `order_by_uses_nocase_collation_when_declared` — mixed-
   case TEXT values sort interleaved when the column was
   declared `COLLATE nocase`.
5. `order_by_default_binary_collation_sorts_uppercase_before_lowercase`
   — default collation sorts ASCII uppercase ahead of
   lowercase.
6. `collate_binary_is_explicit_default_equivalent` —
   explicit `COLLATE binary` produces the same ordering
   as omitting the clause.

Clippy clean on `-D warnings`. All existing tests continue
to compile (TableSpec construction updated via
`..Default::default()` or explicit
`column_collations: Default::default()`).

## 4. BRADIAN_SQL_SUBSET.md impact

Section 3 (DDL) updated to show the full column grammar
with the optional `COLLATE <name>` clause.
