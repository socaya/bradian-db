# ADR 0347 — Engine-sourced column types in `SqlResultSet`

## Status

Accepted. Removes the ADR 0340 wire-side row inference
step from the SELECT paths where the engine can source
types from the catalog directly. Inference stays as a
scoped fallback for SELECT shapes the engine can't yet
type (aggregates, scalar functions, set operators).

## Context

ADR 0340 inferred per-column `WireType` at the wire
handler by scanning the first non-null cell per column.
Two concrete problems:

1. **Empty row sets.** A SELECT that returns 0 rows
   has nothing to infer from, so every column reported
   `WireType::Unknown`. That's wrong — the catalog
   knows the column types regardless of row count.
2. **All-null columns.** A column that happens to be
   NULL in every row was reported as `Unknown`, again
   even though its catalog `DataType` is known.

ADR 0338 added the typed wire row protocol; this slice
completes the loop by having the engine carry typed
column metadata through the result set instead of asking
the wire layer to re-derive it from the payload.

## Decision

Extend `bradian_relational::sql::SqlResultSet::Rows` with
a third field:

```rust
pub enum SqlResultSet {
    Command { affected: u64 },
    Rows {
        columns:      Vec<String>,
        rows:         Vec<Vec<Value>>,
        column_types: Vec<crate::model::DataType>,
    },
}
```

Ordered after `rows` so pre-0347 destructure patterns
of the form `{ columns, rows }` keep working with a
trailing `..` (the migration touches ~115 call sites —
all shorthand-construction sites move to
`column_types: Vec::new()`; all pattern-match sites move
to `..`).

### Engine-side population

The SELECT paths populate `column_types` directly from
the catalog `Table::columns[idx].data_type` when every
projection is a bare column reference. The sites that
source types:

| Path                                                    | Site                                            |
|---------------------------------------------------------|-------------------------------------------------|
| `sql::finish_dml_result_with_table` (DML `RETURNING`)   | positions → `table.columns[*p].data_type`       |
| `sql::execute_impl_single` — bare SELECT                | bare `Projection::Column` → `locate` + lookup   |
| `sql::evaluate_grouped` (GROUP BY, non-aggregate items) | group-key column lookup                         |
| `sql::execute_inner_join`                               | `proj_positions` → `left/right.columns[*pos]`   |
| `sql::transact_table` SELECT                            | positions → `self.table.columns[*p].data_type`  |
| `history_sql::HistorySqlSession::SELECT`                | identical bare-column fast path                 |
| `domain_sql::DomainSqlSession::SELECT`                  | identical bare-column fast path                 |
| `mvcc_sql::MvccSqlSession::SELECT`                      | identical bare-column fast path                 |

Projections that are JSON paths, scalar functions,
aggregates, qualified `alias.col` in a joined non-group
SELECT, or exotic things the engine can't yet fingerprint
all leave `column_types` empty. The wire layer notices
the length mismatch and falls back to ADR 0340 inference
for just those paths.

### Wire-side consumption

`crates/bradian-serving/src/wire.rs`:

1. Destructure captures `column_types: engine_types`.
2. If `engine_types.len() == columns.len()`, project
   each `DataType` to `WireType` via
   `wire_type_from_data_type` (a total function covering
   every catalog `DataType` variant).
3. Else fall back to `infer_column_types(&columns, &rows)`
   (ADR 0340 behaviour) so unknown engine shapes still
   emit something useful.

## Scope

**In scope:**
- New `column_types: Vec<DataType>` field on
  `SqlResultSet::Rows`.
- Catalog-sourced population across all seven SELECT
  paths listed above.
- `wire_type_from_data_type` DataType→WireType total
  function covering all 9 catalog `DataType` variants
  (`Boolean`, `Int32`, `Int64`, `Decimal`, `Varchar`,
  `Text`, `TimestampTz`, `Uuid`, `Json`).
- Wire handler prefers engine-sourced types over row
  inference.
- 3 new tests (62 wire tests total):
  1. `wire_type_from_data_type_covers_every_catalog_variant`
     — the DataType→WireType table is exhaustive and
     stable.
  2. `rowset_column_types_come_from_engine_not_inference_for_empty_bare_select`
     — a 0-row bare-column SELECT reports catalog-
     typed columns (not `Unknown`).
  3. `rowset_column_types_fall_back_to_inference_when_engine_cannot_source`
     — `COUNT(*)` still reports `Int64` via the ADR
     0340 inference fallback.

**Out of scope** (dedicated follow-ups):
- **Aggregate column typing** — the GROUP BY output
  site populates types for group-key columns only;
  aggregate columns (COUNT, SUM, AVG, MIN, MAX) and
  scalar functions fall back to inference. A principled
  type derivation (COUNT → Int64, MIN/MAX → source
  type, SUM over int → Decimal, AVG → Decimal) is the
  next slice.
- **JSON-path projection typing** — ADR 0188 cast-free
  JSON path yields loosely-typed values. An ADR-level
  decision about what WireType to report is pending.
- **Set-operator result typing** — UNION/INTERSECT/
  EXCEPT currently leave `column_types` empty and fall
  back to row inference. Populating from the first arm
  (and type-checking against subsequent arms) is a
  separate correctness question.
- **Qualified joined SELECT projections** — the join
  grouped path accepts `alias.col` which the current
  type-source heuristic rejects. The INNER/OUTER non-
  grouped path already handles this via
  `proj_positions`.

## Consequences

- Clients that depend on `WireType` metadata (ADR 0338
  typed wire rows) now see the catalog's declared type
  even on empty / all-null result sets.
- The `#[non_exhaustive]` nature of `SqlResultSet::Rows`
  is tightened: every consumer of the enum must now be
  aware of `column_types`. The migration touched
  ~115 call sites across `src/` and `tests/`; all
  pattern matches use `..` and all shorthand
  construction sites pass `Vec::new()` with an explicit
  "engine doesn't know" semantic.
- Workspace is green under
  `cargo clippy --workspace --all-targets --locked
  -- -D warnings`. 62 wire tests pass.
- Pre-existing test failure in
  `base_expression_bundle::cast_bad_target_rejected`
  is independent of this slice (reproduces on
  `main` without 0347 — tracked separately).
