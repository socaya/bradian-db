# Master Gate 1 — FOREIGN KEY RESTRICT verification

21 November 2026. ADR 0184. First SQL-engine wiring of the
deferred ADR 0101 §2 FK enforcement. SqlEngine only.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test foreign_key_sql` | 10 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0101 §2 explicitly deferred FK enforcement. ADR 0113
§2 called out the FK action keywords (`CASCADE`,
`SET NULL`, `RESTRICT`) as unparsed. The catalog had the
model (`Table.foreign_keys`) and the full `add_foreign_key`
validation but no path from SQL to populate it, and
`typed_table::validate_table` refused any table with FKs.

Change (ADR 0184):

- Parser accepts
  `FOREIGN KEY (child_cols) REFERENCES <qualified>
  (parent_cols)` at the table level in CREATE TABLE.
- After `catalog.create_table` allocates the child,
  parser resolves each parent qualified name via the
  catalog snapshot and calls the existing
  `catalog.add_foreign_key(child_id, spec)`. All the
  arity/type/target validation lives there.
- `typed_table::validate_table` relaxed to accept FKs
  (FOREIGN KEY sentence removed; UNIQUE / FK are now
  both enforced at the SQL engine layer).
- Two new helpers in `sql.rs`:
  `foreign_key_check(engine, &table, &row)` scans each
  FK's parent table for a match; `foreign_key_restrict(
  engine, &parent, &parent_pk)` scans every child table
  whose FK points at this parent for a reference to the
  departing PK.
- Wiring:
  - `SqlEngine::execute_impl` INSERT → `check_row` →
    `foreign_key_check` → UNIQUE → insert.
  - UPDATE → `check_row(mutated)` →
    `foreign_key_check(mutated)` → UNIQUE(self-filter)
    → replace.
  - DELETE → row exists → `foreign_key_restrict` →
    delete.
- New error: `SqlError::ForeignKey { constraint,
  reason }` with `reason` ∈ `"missing-parent"` /
  `"restrict"`.

Added tests (`tests/foreign_key_sql.rs`, 10 total):
accept + missing-parent at INSERT, FK UPDATE collision,
RESTRICT at DELETE, cleared RESTRICT after children
removed, NULL FK column disables, composite partial
mismatch, restart persistence, unknown parent at
CREATE, arity mismatch at CREATE.

## Scope limits (honest)

- `SqlEngine` only. `DomainSqlEngine` /
  `HistorySqlEngine` / `MvccSqlEngine` FK enforcement
  deferred to a follow-up (mirrors ADR 0178 / 0179
  pattern).
- RESTRICT only. CASCADE / SET NULL / NO ACTION
  deferred to ADR 0185.
- Auto-generated FK constraint names only;
  `CONSTRAINT name FOREIGN KEY ...` syntax not
  parsed.
- Full scan per FK check (O(N) per INSERT/UPDATE
  across each FK's parent; O(N) per DELETE across
  each referencing child). Disk-backed indexes
  remain a Gate 3 prerequisite.

## Evidence

- Source:
  `crates/bradian-relational/src/sql.rs` — FK parser
  clause, `foreign_key_check`, `foreign_key_restrict`,
  `SqlError::ForeignKey`, INSERT/UPDATE/DELETE
  wiring.
  `crates/bradian-relational/src/typed_table.rs` —
  `validate_table` relaxed on FKs.
- Tests:
  `crates/bradian-relational/tests/foreign_key_sql.rs`.
- ADR: `docs/adr/0184-foreign-key-restrict.md`.
