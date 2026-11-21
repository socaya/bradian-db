# Master Gate 1 — `COLLATE` clause verification

21 November 2026. ADR 0175. BRD-SQL-006 collation-at-DDL
closure for the Gate 1 subset; **does not** claim Unicode
UCA or ICU conformance.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test collation_sql` | 6 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0103 shipped the `Collation` primitive
(`Binary` + `CaseInsensitive`) and `Table.column_collations`
has existed since ADR 0113, but no parser hook-up or
downstream usage was wired. The practical effect: callers
could not declare a column's collation from SQL, and the
`select_window` ORDER BY path always compared bytes.

Change (ADR 0175):

- `TableSpec` extended with
  `column_collations: BTreeMap<String, Collation>`
  (`#[serde(default, skip_serializing_if = empty)]` —
  no persistent-format break).
- `PrimaryKey`, `TableSpec` now derive `Default`.
- SQL parser (`SqlEngine::execute` CREATE TABLE path)
  accepts `COLLATE binary` and `COLLATE nocase` after
  the nullability modifier. Unknown names →
  `SqlError::Unsupported`.
- Catalog `create_table` validates each key names an
  existing column with a Text or Varchar type; non-text
  → `CatalogError::InvalidDefinition` surfacing as
  `SqlError::Catalog`.
- `select_window::SortKey` stores the per-column
  collation looked up at parse time; non-text columns
  resolve to `Binary`.
- `select_window::compare` applies the collation to
  `Value::String` / `Value::Json` pairs during ORDER BY.
  Primary-key tie-break stays `Binary` so logical ties
  remain deterministic.
- BRADIAN_SQL_SUBSET.md §3 updated with the new column
  grammar.

Added tests
(`crates/bradian-relational/tests/collation_sql.rs`,
6 total):

1. `create_table_accepts_collate_binary_and_nocase_on_text_columns`.
2. `create_table_rejects_unknown_collate_name` —
   `icu_root` → `SqlError::Unsupported`.
3. `create_table_rejects_collate_on_non_text_column` —
   COLLATE on BIGINT → `SqlError::Catalog`.
4. `order_by_uses_nocase_collation_when_declared` —
   mixed-case values interleave under `nocase`.
5. `order_by_default_binary_collation_sorts_uppercase_before_lowercase`.
6. `collate_binary_is_explicit_default_equivalent`.

Existing test-construction sites for `TableSpec`
updated to add `column_collations: Default::default()`
(or `..Default::default()` where other defaults apply).

## Scope limits (honest)

- **ASCII case folding only.** `nocase` does not apply
  Unicode UCA / locale folding; non-ASCII bytes compare
  bytewise (documented in ADR 0103 §2).
- **COLLATE applies to ORDER BY only.** Equality / PK /
  UNIQUE comparisons still use byte semantics. A follow-
  up ADR can wire collation into those paths and
  coordinate with the ADR 0127 secondary-index helper.
- **COLLATE on JSON columns is refused.** JSON always
  compares as canonical UTF-8 bytes.
- **No per-query `ORDER BY ... COLLATE <name>`
  override.** Collation is declared at CREATE TABLE and
  used implicitly.

## Evidence

- Source:
  `crates/bradian-relational/src/model.rs` —
  `TableSpec.column_collations`; `Default` derives.
  `crates/bradian-relational/src/catalog.rs` —
  collation validation + persistence into `Table`.
  `crates/bradian-relational/src/sql.rs` — CREATE
  TABLE parser accepting `COLLATE <name>`.
  `crates/bradian-relational/src/select_window.rs` —
  per-sort-key collation and collation-aware `compare`.
- Tests:
  `crates/bradian-relational/tests/collation_sql.rs`
  (6 round-trip + rejection tests).
- ADR: `docs/adr/0175-collate-clause-create-table.md`.
- Reference doc: `docs/reference/BRADIAN_SQL_SUBSET.md`
  §3 updated.
