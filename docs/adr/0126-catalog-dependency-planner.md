# ADR 0126 — Catalog dependency planner (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch B B7 primitive.
Complements ADR 0124 (DDL extensions) by replacing the
current one-reason refusals with a complete dependency
listing a caller can show the operator before attempting
the DDL.

## 1. Scope

Two new pure `Catalog` analysis methods:

- `Catalog::table_dependents(id) -> Vec<Dependency>` —
  every edge that would block `drop_table(id)`. Empty iff
  the drop would succeed.
- `Catalog::column_dependents(id, name) -> Vec<Dependency>` —
  every edge that would block `drop_column(id, name)`.
  Empty iff the drop would succeed.

Both return a Vec; unknown table/column names return an
empty Vec (not an error — the planner is purely
analytical).

New public enum `Dependency`:

| Variant | Meaning |
|---|---|
| `LastColumn` | Dropping leaves the table empty. |
| `PrimaryKey { columns }` | Column is part of the PK. |
| `UniqueKey { constraint, columns }` | Column is in a UK. |
| `ForeignKey { constraint, columns, referenced_table }` | Column is part of a local FK. |
| `IncomingForeignKey { from_table, constraint, columns, referenced_columns }` | Another table's FK references this table/column. |

## 2. Out of scope

- **Views / stored procedures / triggers** — Bradian
  doesn't have them yet; when they land the planner will
  need `View { id, name }` + `Trigger { id, name }`
  variants.
- **Policy references (ADR 0123)** — row/column/tenant
  policies naming a column aren't tracked here because
  policy storage in the catalog is still OPEN.
- **Index references** — the mutable index overlay / B+tree
  doesn't carry per-column identity in the catalog layer.
- **Default / computed-expression references** — future
  DEFAULT-expression ADRs will add
  `DefaultExpression { column }`.
- **Wiring into `drop_table` / `drop_column`** —
  the existing error paths still refuse on the first
  conflict with a `DependentObjects(_)` message. A
  follow-up ADR can route them through the planner so
  the error message carries every reason. Keeping them
  separate for now preserves error-class stability.

## 3. Compliance

- `table_dependents` lists every incoming FK; empty for
  leaf tables; empty for unknown IDs.
- `column_dependents` lists PK / UK / local FK / incoming
  FK / last-column edges; combines multiple categories
  (e.g., PK + incoming FK on same column).
- Local FK and incoming FK reported separately with
  distinct variants.
- Idempotent / pure: 10× invocation leaves the catalog
  unchanged (direct test).
- 9 new tests in `catalog_dependency_planner.rs`.

## 4. Honest status

Foundation only. The drop paths still raise the
single-reason error; callers that want a complete list
call the planner first. A future ADR can replace the
`DependentObjects(String)` tuple with
`DependentObjects(Vec<Dependency>)` across every error
site — but that is a breaking error-type change requiring
cross-crate updates.
