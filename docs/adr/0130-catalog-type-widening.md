# ADR 0130 — Catalog type-widening primitive (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-16.** Batch B B6 primitive.
Companion to ADR 0124 (DDL extensions) and ADR 0126
(dependency planner). Only strictly additive widenings are
permitted — no reinterpret, no scale change.

## 1. Scope

New method `Catalog::widen_column_type(id, column, new_type)`.

**Permitted widenings:**

| From | To | Rationale |
|---|---|---|
| `Int32` | `Int64` | Value range is a strict superset. |
| `Varchar(N)` | `Varchar(M)` where M > N | Length ceiling grows. |
| `Varchar(N)` | `Text` | Text is unlimited. |
| `Decimal(p, s)` | `Decimal(p', s)` where p' > p | More digits left of decimal; scale unchanged. |

Same-type target is a no-op (`Ok(())`).

**Rejected:**

- Narrowing (any direction).
- `Text → Varchar` (would require length check).
- Scale change on `Decimal` (would shift the decimal point
  of stored values).
- Cross-family (`Int64 → Decimal`, `Int32 → Text`, …).
- Widening on any column with a dependency edge
  (consulted via ADR 0126 `column_dependents`): PK, UK,
  local FK, incoming FK. The planner would need to walk
  dependent constraints before a reference type can change.

## 2. Out of scope (follow-ups)

- **Row-domain rewrite** — this is a Catalog-metadata-only
  widening. Stored rows are not re-encoded; readers are
  assumed to tolerate the wider type. Int32 → Int64
  specifically needs the typed-row decoder to accept both
  widths before this is safe to publish durably.
- **PersistentCatalog journaling** — no WAL event emitted.
- **SQL parser `ALTER COLUMN TYPE`** — parser refuses this
  form; callers invoke the mutator programmatically.
- **Cascading widening** — widening a PK requires widening
  every referencing FK atomically. Deferred to the
  dependency-planner-driven DDL coordinator.
- **Decimal scale increase with rewrite** — would require
  per-row conversion.
- **Type-widening on columns with DEFAULTs** — the stored
  default is not re-coerced; `Value::Int32(5)` under an
  `Int64` column would need promotion.

## 3. Compliance

- Int32 → Int64 accepted.
- Varchar length increase accepted.
- Varchar → Text accepted.
- Decimal precision increase (scale unchanged) accepted.
- Same-type no-op accepted.
- Int64 → Int32 narrowing rejected.
- Varchar length decrease rejected.
- Text → Varchar rejected.
- Decimal scale change rejected.
- Decimal precision decrease rejected.
- PK / UK / incoming-FK columns rejected with
  `DependentObjects` or `InvalidDefinition`.
- Unknown column / unknown table rejected with
  `NotFound`.
- 15 new tests in `catalog_type_widening.rs`.

## 4. Honest status

Foundation only. **BRD-DDL-004 type-widening** is not yet
satisfied because the row-domain decoder, FK cascading
widening, DEFAULT re-coercion, parser wiring, and
durable journaling all remain OPEN. The Catalog can
advertise a wider type but readers must continue to
accept both old and new representations until a rewrite
pass runs.
