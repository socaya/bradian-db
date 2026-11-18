# ADR 0166 — ALTER COLUMN SET/DROP DEFAULT + TYPE DDL (**ACCEPTED**)

**Status: ACCEPTED on 2026-11-18.** Companion to
ADR 0165, exposing the remaining catalog mutators
(ADR 0124 defaults, ADR 0130 widening) as journaled
SQL DDL.

## 1. Scope

Three new PersistentCatalog `Command` variants:
`SetColumnDefault { table, column, value }`,
`DropColumnDefault { table, column }`,
`WidenColumnType { table, column, new_type }`. Each
applies via the matching `Catalog::*` mutator with
full type validation.

SQL parser additions:
```
ALTER TABLE <q> ALTER COLUMN <col> SET DEFAULT <literal>
ALTER TABLE <q> ALTER COLUMN <col> DROP DEFAULT
ALTER TABLE <q> ALTER COLUMN <col> TYPE <data_type>
```

Literal defaults route through the existing `coerce`
pipeline. Widening is strictly additive per ADR 0130:
Int32 → Int64, Varchar(N) → Varchar(M>N) / Text,
Decimal(p,s) → Decimal(p'>p, s).

## 2. Out of scope

- **DEFAULT expressions** — only literal defaults.
  ADR 0136 `DefaultExpr` catalog migration remains
  OPEN.
- **Type narrowing** — rejected at the Catalog layer.
- **Online ALTER COLUMN TYPE** with row-level
  re-encoding — caller does offline rewrite today.

## 3. Compliance

Covered in `alter_table_sql.rs` alongside ADR 0165:
set_default stores value, drop_default idempotent,
type widen succeeds.
