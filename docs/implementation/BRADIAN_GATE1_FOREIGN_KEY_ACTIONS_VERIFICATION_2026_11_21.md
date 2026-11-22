# Master Gate 1 — FOREIGN KEY `ON DELETE` actions verification

21 November 2026. ADR 0185. Follow-up to ADR 0184 FK
RESTRICT; closes the ADR 0113 §2 FK action keyword
follow-up.

## Executed result

| Executed check | Result |
|---|---|
| `cargo test --locked -p bradian-relational --test foreign_key_actions_sql` | 8 passed, 0 failed |
| `cargo test --locked -p bradian-relational --test foreign_key_sql` | 10 passed, 0 failed (ADR 0184 regression) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## Finding and action

ADR 0184 shipped only RESTRICT (default). Operators who
wanted CASCADE or SET NULL could not express the
intent. ADR 0113 §2 listed these keywords as unparsed.

Change (ADR 0185):

- New `ReferentialAction { Restrict, Cascade, SetNull }`
  enum on `model::ForeignKey` and `model::ForeignKeySpec`
  (default Restrict; serde skip-if-restrict preserves
  pre-ADR-0185 bytes).
- Parser: `ON DELETE <action>` and `ON UPDATE <action>`
  clauses after `FOREIGN KEY ... REFERENCES ...`. Actions:
  CASCADE, SET NULL, RESTRICT, NO ACTION (NO ACTION
  behaves as RESTRICT in this scope).
- Catalog `add_foreign_key` refuses SET NULL when the
  child FK column is NOT NULL (fail-fast at CREATE).
- `sql::foreign_key_restrict` branches on `on_delete`:
  - RESTRICT — unchanged from ADR 0184.
  - CASCADE — delete matching children by PK;
    recursively re-invokes `foreign_key_restrict` so
    grandchildren honor their own actions; any
    downstream RESTRICT aborts the whole chain.
  - SET NULL — set FK columns to NULL on matching
    children and `replace`; re-runs CHECK / FK / UNIQUE
    on the mutated row.
- ON UPDATE field preserved but inert (SqlEngine
  rejects PK UPDATE unconditionally).

Added tests
(`crates/bradian-relational/tests/foreign_key_actions_sql.rs`,
8 total): CASCADE removes matching children, CASCADE
only matching (sibling survives), SET NULL nulls the
column, SET NULL rejected at CREATE on NOT NULL child,
explicit RESTRICT equivalence, NO ACTION equivalence,
transitive CASCADE across three tables, RESTRICT on
grandchild aborts the chain.

Existing ADR 0184 FK test suite (10 tests) continues to
pass — default behavior is RESTRICT, so no regressions.

## Scope limits (honest)

- `SqlEngine` only. `DomainSqlEngine` /
  `HistorySqlEngine` / `MvccSqlEngine` FK and action
  wiring remains open.
- ON UPDATE is parsed and stored but inert because
  PK UPDATE itself remains unsupported.
- No deferrable / initially deferred semantics.
- Auto-generated FK names only.

## Evidence

- Source:
  `crates/bradian-relational/src/model.rs` — new
  `ReferentialAction` enum + fields on `ForeignKey` /
  `ForeignKeySpec`.
  `crates/bradian-relational/src/catalog.rs` — SET
  NULL nullable-column refusal + action persistence.
  `crates/bradian-relational/src/sql.rs` — parser
  clause, `parse_referential_action` helper, action-
  aware `foreign_key_restrict`.
- Tests:
  `crates/bradian-relational/tests/foreign_key_actions_sql.rs`.
- ADR: `docs/adr/0185-foreign-key-actions.md`.
