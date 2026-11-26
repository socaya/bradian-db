# ADR 0352 — Per-column type agreement in set operators

## Status

Accepted. Tightens ADR 0349 by requiring element-wise
type agreement across set-operator arms before engine
types are carried forward. Previously the accumulator
retained arm 1's types even when arm 2 declared a
different `DataType` for the same column.

## Context

ADR 0349 carried the first arm's `column_types` through
a UNION / UNION ALL / INTERSECT / EXCEPT, clearing the
vec only when a subsequent arm's `column_types.len()`
didn't match the accumulator's. That check caught the
"arm 2 doesn't source types at all" case but missed
"arm 2 sources types of a different shape" — e.g.

```sql
SELECT value FROM t.public.items UNION ALL
SELECT CAST(value AS TEXT) FROM t.public.items
```

Here both arms report typed columns, both have exactly
one column, but the first is `Int64` and the second is
`Text`. ADR 0349 would carry `[Int64]` forward and the
wire layer would advertise `WireType::Int64` even
though half the rows in the stream are `WireValue::String`
cells. That's a lie to the client.

## Decision

Every set-operator type-agreement check now also
compares per-column types element-wise:

```rust
if next_types.len() != column_types.len()
    || next_types.iter().zip(&column_types)
        .any(|(a, b)| a != b)
{
    column_types.clear();
}
```

Four sites updated — identical pattern in each:

- `sql::SqlEngine::execute_impl` (UNION fast path)
- `sql::SqlEngine::execute_set_ops` (INTERSECT / EXCEPT)
- `history_sql::HistorySqlEngine::execute` (UNION)
- `domain_sql::DomainSqlEngine::execute` (UNION)
- `mvcc_sql::MvccSqlEngine::execute` (UNION)

Clearing the accumulator pushes the whole result back
to ADR 0340 row-based inference — the conservative
honest choice. Cross-arm type promotion (e.g.
`Int32 ⊕ Int64 → Int64`) would require a rules table
with rejection for incompatible pairs; that is a
larger ADR covering widening semantics across every
`DataType` pair and is explicitly left to a later
slice.

## Scope

**In scope:**
- Element-wise agreement check in every set-operator
  type-propagation guard across the four SQL engines.
- 2 new tests (72 wire tests total):
  1. `union_with_disagreeing_per_column_types_drops_to_inference`
     — a mixed `[Int64]` vs `[Text]` UNION ALL no
     longer advertises engine types; the wire layer
     falls back to row inference.
  2. `union_with_matching_per_column_types_keeps_engine_types`
     — two identical bare-column arms with zero rows
     still report `[Int64]` engine-sourced (proving
     the tightened check doesn't over-trigger).

**Out of scope** (dedicated follow-up):
- **Cross-arm type promotion / rejection.** A principled
  compatibility matrix across every `DataType` pair
  (promote `Int32` + `Int64` → `Int64`; reject
  `Boolean` ⊕ `Int64`; preserve `Decimal` scale rules)
  is a separate correctness ADR.

## Consequences

- A set-operator result never advertises a column
  type its payload may not satisfy. Clients get the
  catalog-declared type when every arm agrees, and
  inference-sourced best-effort shape when they
  disagree.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 72 wire tests pass.
- Pre-existing `base_expression_bundle::cast_bad_target_rejected`
  failure remains unrelated.
