# ADR 0124 — Catalog DDL extensions: RENAME / DROP COLUMN / SET-DROP DEFAULT (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch B software-only
gaps B2 + B4 + B5. SET NOT NULL + backfill (B3), DROP NOT
NULL in-place (already in G39/ADR 0034), DEFAULT expressions
(B1), type-widening (B6) and a full dependency planner (B7)
remain follow-up ADRs.

## 1. Scope

Five new in-memory `Catalog` mutation primitives:

| Method | Behavior |
|---|---|
| `rename_table(id, new_name)` | Preserves `TableId`; enforces identifier + per-schema uniqueness. |
| `rename_column(id, old, new)` | Updates column name, PK columns, UK columns, local FK columns, incoming FK `referenced_columns`, `column_defaults` key, `column_collations` key. |
| `drop_column(id, name)` | Rejects PK columns, UK columns, local FK columns, incoming-FK targets, last column. |
| `set_column_default(id, name, value)` | Validates against declared type via `default_matches_type` (subset of typed-table validator); NULL rejected (= DROP DEFAULT). |
| `drop_column_default(id, name)` | Idempotent remove. |

## 2. Out of scope (next ADRs)

- **PersistentCatalog journaling** — these mutators run on
  the in-memory `Catalog` only. Durable DDL events for
  RENAME / DROP COLUMN / SET-DROP DEFAULT are not emitted;
  replay is not yet tested.
- **SQL parser `ALTER TABLE RENAME / DROP COLUMN / ALTER
  COLUMN SET DEFAULT / DROP DEFAULT`** — the native SQL
  parser (ADR 0017 family) still refuses these forms.
  Callers invoke the methods programmatically.
- **HistorySqlEngine schema-reconciliation** — retained
  historical rows under the OLD schema are not projected
  through a RENAME or DROP COLUMN. G36/G37 ADR 0031-0032
  schema-aware AS OF coverage expands separately.
- **Row-domain backfill** — DROP COLUMN does not rewrite
  stored rows; unused column slots remain in historical
  pages until the next catalog snapshot rewrite (ADR 0119).
- **DEFAULT expressions** — SET DEFAULT stores a literal
  `Value` only. `now()`, `uuid_generate_v4()` and other
  scalar-expression defaults need a parser pass.
- **Type widening** — `ALTER COLUMN ... TYPE <new>` with
  subset validation + reversible conversion.
- **Dependency planner** — a single refusal error today;
  future planner should list every dependent constraint /
  view / FK path before refusing.

## 3. Compliance

- RENAME TABLE: renames in place; duplicate target refused.
- RENAME COLUMN: updates columns + PK + UK; unknown column
  refused; duplicate target refused.
- RENAME COLUMN cascades to incoming FK `referenced_columns`.
- DROP COLUMN: removes non-key column; PK / UK / local FK /
  incoming FK / last-column all refused with distinct error
  classes.
- SET DEFAULT: type-matches against column; mismatches and
  NULL refused; unknown column refused.
- DROP DEFAULT: idempotent; unknown column refused.
- 17 new tests in `catalog_ddl_extensions.rs`.

## 4. Honest status

This is a **foundation** ADR in the ADR 0096 / 0098
convention: narrow primitives with explicit callouts.
It does not claim BRD-DDL-002 compliance, which requires
parser wiring, durable journaling, retained-history schema
projection and an upgrade-compatibility suite.
