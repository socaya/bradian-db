# BACE SQL engines open verification (ADR 0071)

15 November 2026. Baseline: `544100d`. Thin operator-facing
derivatives of the coordinator work in ADRs 0069 and 0070. The
three SQL engines operators actually touch now expose BACE
entry points.

## Executed result

**970 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 970 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/src/typed_table.rs`

```rust
impl TableStore {
    pub fn open(path, table);
    pub fn open_with_bace(path, table, wiring: BaceWiring);
    fn open_inner(path, table, wiring: Option<BaceWiring>);
}
```

Shared body uses `RowStore::open_with_bace` when wiring is
present; the META_KEY schema-pin reconciliation that follows the
storage open is unchanged.

### `crates/bradian-relational/src/domain_sql.rs`

```rust
impl DomainSqlEngine {
    pub fn open_with_bace(root, wiring);
    pub fn open_with_bace_and_limits(root, wiring, domain_limits, catalog_limits);
    fn open_inner(root, domain_limits, catalog_limits, wiring: Option<BaceWiring>);
}
```

Both component stores (`PersistentCatalog`, `MultiTableStore`)
are opened eagerly under the shared wiring; no wiring stored on
the engine.

### `crates/bradian-relational/src/sql.rs`

```rust
pub struct SqlEngine {
    root: PathBuf,
    catalog: PersistentCatalog,
    operation: Mutex<()>,
    bace_wiring: Option<BaceWiring>,     // NEW
}

impl SqlEngine {
    pub fn open(root);
    pub fn open_with_bace(root, wiring);
    fn open_inner(root, wiring: Option<BaceWiring>);

    fn table_store(&self, table) -> SqlResult<TableStore> {
        let path = self.root.join("tables").join(table.id.0.to_string());
        Ok(match &self.bace_wiring {
            Some(w) => TableStore::open_with_bace(path, table.clone(), w.clone())?,
            None    => TableStore::open(path, table.clone())?,
        })
    }
}
```

The lazy `table_store()` call is the only mutation; it threads
the wiring to every per-table `TableStore::open_with_bace`.

### 5 new tests

| # | Test | ADR §3.5 item |
|---|---|---|
| 1 | `table_store_open_with_bace_round_trips_a_typed_put_get_commit_cycle` | 1 |
| 2 | `domain_sql_engine_open_with_bace_round_trips_ddl_and_dml` | 2 |
| 3 | `domain_sql_engine_open_with_bace_tolerates_legacy_v1_only_directory` | 6 |
| 4 | `sql_engine_open_with_bace_round_trips_ddl_and_dml` | 3 |
| 5 | `sql_engine_open_with_bace_tolerates_legacy_v1_only_tables` | 7 |

### §3.5 items 4 and 5 observed behavior

The drafted §3.5 items 4 and 5 (legacy open on a BACE-written
root fails closed) could not be exercised with today's fixtures
for the same reason as ADR 0070 §6 item 6: fresh opens without
further mutations before reopen never emit v2 pages (ADR 0065
"checkpoint idempotent at current LSN" rule). The RowStore-
layer fail-closed invariants are covered by ADR 0067's test
suite and inherit transitively through both SQL engines. The
shipped tests cover the still-reachable positive paths (ADR
0065 §3.5 mixed-generation tolerance via items 3, 6, 7).

## Compliance against ADR 0071 §3.5

| Item | Status |
|---|---|
| 1 `TableStore::open_with_bace` round-trips | ✓ test 1 |
| 2 `DomainSqlEngine::open_with_bace` full DDL + DML | ✓ test 2 |
| 3 `SqlEngine::open_with_bace` DDL + per-table DML | ✓ test 4 |
| 4 Legacy `DomainSqlEngine::open` on BACE directory fails closed | Documented; inherit from RowStore layer (ADR 0067 test suite) |
| 5 Legacy `SqlEngine::open` lazy failure on BACE table | Documented; inherit from RowStore layer |
| 6 `DomainSqlEngine::open_with_bace` on v1-only dir | ✓ test 3 |
| 7 `SqlEngine::open_with_bace` on v1-only per-table dirs | ✓ test 5 |

## What this ADR explicitly does NOT do (unchanged)

- Touch any on-disk format.
- Add new policy keys or codecs.
- Change coordinator backup paths.
- Touch `MvccSqlEngine` / `TypedMvccStore` / `MvccStore`.
- Touch G23 DDL coordinator flows.
- Introduce archive-level compression, encryption, or
  authentication.
- Change per-table directory layout.
- Change the operation-mutex / lock discipline.

## Known scope limits recorded for follow-up ADRs

- **`MvccSqlEngine::open_with_bace`.** G21 in-memory MVCC
  engine. Requires `MvccStore::open_with_bace` and
  `TypedMvccStore::open_with_bace` as prerequisites.
- **G23 DDL coordinator BACE**
  (`enable_epoch_row_writes`, `execute_ddl`,
  `activate_schema`). Would also unblock the observed scope
  limit from ADR 0070 §6 item 6 (actual v2 pages in a G24
  coordinator).
- **Authenticated envelope (`BACA`).** Different family of
  work; separate ADR.
- **`.derc` carrier BACE.** Analytical column segments —
  orthogonal carrier; separate ADR.
- **BRWL carrier BACE.** WAL frames — separate ADR.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067 | Backup carrier adapter | ACCEPTED |
| 0068 | Coordinator backup carrier adapter (restore) | ACCEPTED |
| 0069 | CatalogLifecycle BACE open (G19 writer) | ACCEPTED |
| 0070 | HistorySqlEngine BACE open (G24 writer) | ACCEPTED |
| **0071** | **SQL engines BACE open (operator-facing)** | **ACCEPTED** |

The three SQL engines most operators actually open
(`SqlEngine`, `DomainSqlEngine`, and `HistorySqlEngine` via ADR
0070) all now accept BACE wiring. `MvccSqlEngine` remains the
one SQL engine without BACE opens.

## Baseline + commit

Baseline: `544100d` (ADR 0071 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0071](../adr/0071-sql-engines-bace-open.md).
