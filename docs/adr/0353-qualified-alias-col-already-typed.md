# ADR 0353 — Qualified `alias.col` in grouped joins is already engine-typed (correcting ADR 0350)

## Status

Accepted. Corrects a false "follow-up" claim in ADR
0350 by demonstrating via a regression test that
qualified column references in grouped join SELECTs
**already** produce engine-sourced `column_types`
at the wire boundary. No code change is required.

## Context

ADR 0350 — "Engine-sourced column types for scalar
functions" — listed as explicit out-of-scope:

> **Qualified `alias.col` in grouped join SELECT** —
> the last `name.contains('.')` short-circuit in the
> type loop. Resolving against the composite schema
> in `execute_join_grouped` is wiring already done
> for the non-grouped INNER/OUTER JOIN path.

That claim was incorrect. The `name.contains('.')`
short-circuit at `crates/bradian-relational/src/sql.rs`
only guards the **single-table** SELECT type-source
loop. The grouped-join dispatch reaches
`evaluate_grouped(composite, ...)` with a synthetic
**composite** table whose columns are literally named
`alias.col` (see `build_composite_table` — each
column's name is built as `format!("{alias}.{}", c.name)`).

The type-source loop inside `evaluate_grouped` calls
`locate(table, name)` — `locate` does an exact-name
lookup against `table.columns`. When `table` is the
composite, `locate(composite, "a.id")` resolves
directly to the catalog-declared `DataType` of the
left table's `id` column. No dot-stripping is needed.

## Decision

Document the current behaviour with a dedicated
regression test and update the record. No library
change.

New test `grouped_join_qualified_select_reports_engine_types`
in `crates/bradian-serving/src/wire.rs`:

```sql
CREATE TABLE t.public.items2 (id BIGINT NOT NULL, note BIGINT, PRIMARY KEY(id));
INSERT INTO t.public.items2(id, note) VALUES(1, 42);
INSERT INTO t.public.items2(id, note) VALUES(2, 43);

SELECT a.id, COUNT(*)
FROM t.public.items  AS a
JOIN t.public.items2 AS b ON a.id = b.id
GROUP BY a.id;
```

Asserts the wire response is a `RowSet` with
`column_types == [WireType::Int64, WireType::Int64]`
— qualified column plus aggregate, both engine-sourced.

## Scope

**In scope:**
- Regression test pinning the grouped-join-with-
  qualified-column type-source path.
- Written record correcting the ADR 0350 follow-up
  list.

**Out of scope** (unchanged from earlier ADRs):
- Cross-arm set-operator type promotion (ADR 0352
  follow-up).
- End-to-end mid-scan cancellation test (ADR 0351
  follow-up).
- Client-side statement deadline wiring (ADR 0351
  follow-up).
- Session-engine scalar function support (ADR 0350
  follow-up — the three session engines don't accept
  `SelectItem::Func` today).
- Bare `Decimal` literal precision/scale inference
  (ADR 0350 follow-up).

## Consequences

- One regression test guards the behaviour so future
  refactors of `build_composite_table` /
  `evaluate_grouped` / `execute_join_grouped` don't
  silently regress to inference-sourced types.
- The ADR 0350 follow-up list is now one item
  shorter.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 73 wire tests pass.
- Pre-existing `base_expression_bundle::cast_bad_target_rejected`
  failure remains unrelated.
