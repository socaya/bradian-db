# ADR 0165 — ALTER TABLE RENAME / DROP COLUMN DDL (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Promotes the Batch B
catalog mutators (ADR 0124) from a programmatic API to
first-class SQL DDL journaled durably through
PersistentCatalog.

## 1. Scope

Three new PersistentCatalog `Command` variants:
`RenameTable`, `RenameColumn`, `DropColumn`. Each applies
via the matching `Catalog::*` mutator, inheriting its
validation + dependency checks (ADR 0124 + 0126
planner).

Public methods on `PersistentCatalog`:
- `rename_table(id, new_name)`.
- `rename_column(id, old, new)`.
- `drop_column(id, column)`.

New SQL parser dispatch arm `"alter"` on `SqlEngine`:
```
ALTER TABLE <qualified> RENAME TO <new_name>
ALTER TABLE <qualified> RENAME COLUMN <old> TO <new>
ALTER TABLE <qualified> DROP COLUMN <column>
```

## 2. Out of scope

- **Session-engine DDL** (`HistorySqlEngine`,
  `DomainSqlEngine`, `MvccSqlEngine`) — the session
  engines already route catalog changes through their
  own lifecycle machinery.
- **Cross-catalog RENAME** — same-schema only.
- **Lifecycle reconciliation** of retained history
  rows across RENAME / DROP — ADR 0126 planner
  already rejects dependent columns.

## 3. Compliance

New integration test file `alter_table_sql.rs` (11
tests): RENAME TABLE / RENAME COLUMN / DROP COLUMN
each with success + error paths + persistence across
engine reopen + PK-drop rejection + unknown-table
rejection.
