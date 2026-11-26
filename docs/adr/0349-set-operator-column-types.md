# ADR 0349 — Engine-sourced column types for set operators

## Status

Accepted. Extends ADR 0347 / 0348 to the UNION /
UNION ALL / INTERSECT / EXCEPT paths across the four
SQL engines, closing the last "falls back to inference"
case the previous two ADRs explicitly deferred.

## Context

ADR 0347 landed engine-sourced `column_types` on bare-
column SELECTs; ADR 0348 extended it to scalar and
grouped aggregates. Both explicitly deferred set
operators:

> **Set-operator result typing** — UNION / INTERSECT /
> EXCEPT currently leave `column_types` empty and fall
> back to row inference. Populating from the first arm
> (and type-checking against subsequent arms) is a
> separate correctness question.

Six sites still returned `column_types: Vec::new()`:

| Site                                             | Operator shape              |
|--------------------------------------------------|-----------------------------|
| `sql::SqlEngine::execute_impl` (UNION fast path) | UNION / UNION ALL ≥ 2 arms  |
| `sql::SqlEngine::execute_set_ops`                | INTERSECT / EXCEPT          |
| `history_sql::HistorySqlEngine::execute`         | UNION / UNION ALL           |
| `domain_sql::DomainSqlEngine::execute`           | UNION / UNION ALL           |
| `mvcc_sql::MvccSqlEngine::execute`               | UNION / UNION ALL           |

Empty-result or all-null set operators reported
`WireType::Unknown` even though both arms declared
typed columns.

## Decision

Every set-operator path now captures the first arm's
`column_types` and threads it through the loop. On
each subsequent arm:

1. If the arm's `column_types.len()` doesn't match the
   accumulator's, clear the accumulator so the whole
   result drops back to row inference at the wire
   boundary. This is deliberately conservative — a
   principled per-column cross-arm type check
   (promotion rules, outright rejection of mismatches)
   is a separate ADR.
2. Otherwise keep the accumulator's types unchanged.
   Since every one of the ~N engine-typed paths
   produces the SAME `DataType` for the SAME column
   reference, this is correct for all current shapes.

The pattern is the same six times:

```rust
let (columns, mut rows, mut column_types) = match first {
    SqlResultSet::Rows { columns, rows, column_types } =>
        (columns, rows, column_types),
    _ => return Err(/* set op requires SELECTs */),
};
for (op, piece) in iter {
    let next = /* execute piece */;
    let (nc, nr, nt) = match next { SqlResultSet::Rows { .. } => /* bind */, _ => err };
    if nc.len() != columns.len() { return err; }
    if nt.len() != column_types.len() { column_types.clear(); }
    /* merge rows per operator */
}
Ok(SqlResultSet::Rows { columns, rows, column_types })
```

## Scope

**In scope:**
- All six set-operator emission sites across the four
  SQL engines carry engine-sourced types from the
  first arm through the final result.
- Conservative drop-to-empty rule when any arm lacks
  typed columns (keeps the wire layer's ADR 0340
  fallback in play for untypable mixes).
- 2 new tests (67 wire tests total):
  1. `union_carries_engine_types_when_every_arm_sources_them`
     — UNION ALL of two bare-column SELECTs reports
     the source column's `WireType::Int64`.
  2. `union_distinct_preserves_engine_types_through_the_dedup_sort`
     — UNION (no ALL) runs dedup + sort after
     merging; types survive the extra step and still
     report `[Int64, Int64]`.

**Out of scope** (dedicated follow-ups):
- **Per-column cross-arm type promotion** — if arm 1
  returns `Int32` and arm 2 returns `Int64`, the
  current implementation keeps arm 1's type (both
  arms must statically agree on the column shape the
  engine returns today). A principled widen/reject
  decision across every `DataType` pair is a
  separate correctness ADR.
- **Scalar function / JSON-path arms** — these drop
  to inference per ADR 0347; the UNION path inherits
  the same behaviour (if any arm is untypable, the
  whole UNION is untypable).

## Consequences

- An empty-result or all-null UNION / UNION ALL /
  INTERSECT / EXCEPT reports the catalog-declared
  column type instead of `WireType::Unknown` for
  every shape where every arm is statically typable.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 67 wire tests pass.
- Pre-existing `base_expression_bundle::cast_bad_target_rejected`
  failure remains unrelated.
