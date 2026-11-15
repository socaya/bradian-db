# BACE MvccSqlEngine open verification (ADR 0072)

15 November 2026. Baseline: `f05188a`. Final SQL engine BACE
open. All four SQL engines now accept `BaceWiring`.

## Executed result

**973 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 973 passed, 0 failed |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/src/mvcc.rs`

```rust
impl MvccStore {
    pub fn open_with_bace(root, wiring);
    pub fn open_with_bace_and_limits(root, wiring, limits);
    fn open_inner(root, limits, wiring: Option<BaceWiring>);
}
```

The only mutation inside `open_inner` is routing to
`RowStore::open_with_bace` when wiring is present. The
in-memory `History` seed afterwards is unchanged.

### `crates/bradian-relational/src/typed_mvcc.rs`

```rust
impl TypedMvccStore {
    pub fn open_with_bace(domain_root, catalog_tables, wiring);
    pub fn open_with_bace_and_limits(domain_root, catalog_tables, wiring, mvcc_limits);
    fn open_inner(domain_root, catalog_tables, mvcc_limits, wiring: Option<BaceWiring>);
}
```

Delegates to `MvccStore::open_with_bace_and_limits` when wiring
is present.

### `crates/bradian-relational/src/mvcc_sql.rs`

```rust
impl MvccSqlEngine {
    pub fn open_with_bace(root, wiring);
    pub fn open_with_bace_and_limits(root, wiring, mvcc_limits, catalog_limits);
    fn open_inner(root, mvcc_limits, catalog_limits, wiring: Option<BaceWiring>);
}
```

The G18 lifecycle pre-flight (when `root/lifecycle` exists)
uses `CatalogLifecycle::open_with_bace(root, wiring.clone())`
when wiring is supplied, so the temporary coordinator reconcile
sees the same wiring the subsequent eager opens use.

No wiring is stored on the engine — G21 has no durable history
and no coordinator backup.

### 3 new tests in `tests/mvcc_sql.rs`

| # | Test | ADR §3.5 item |
|---|---|---|
| 1 | `mvcc_store_open_with_bace_round_trips_a_put_get_commit` | 1 |
| 2 | `mvcc_sql_engine_open_with_bace_round_trips_full_mvcc_session` | 2, 3 (TypedMvccStore exercised transitively) |
| 3 | `mvcc_sql_engine_open_with_bace_tolerates_legacy_v1_only_directory` | 4 |

## Compliance against ADR 0072 §3.5

| Item | Status |
|---|---|
| 1 `MvccStore::open_with_bace` round-trips | ✓ test 1 |
| 2 `TypedMvccStore::open_with_bace` reads schema pins | ✓ (covered transitively by test 2) |
| 3 Full MVCC SQL session INSERT/SELECT | ✓ test 2 |
| 4 Legacy v1-only tolerance | ✓ test 3 |
| 5 G18 pre-flight consistent-wiring | Documented; inherits from ADR 0069 test suite |

## What this ADR explicitly does NOT do (unchanged)

- Touch any on-disk format.
- Add new policy keys or codecs.
- Add backup paths on MvccSqlEngine.
- Touch G23 DDL coordinator flows.
- Touch `.brcs` or BRWL carriers.
- Change in-memory MVCC version semantics.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0066 | Prerequisites + RowStore + ZSTD | ACCEPTED |
| 0067 | Backup carrier adapter | ACCEPTED |
| 0068 | Coordinator backup carrier adapter (restore) | ACCEPTED |
| 0069 | CatalogLifecycle BACE open (G19 writer) | ACCEPTED |
| 0070 | HistorySqlEngine BACE open (G24 writer) | ACCEPTED |
| 0071 | SQL engines BACE open (SqlEngine + DomainSqlEngine) | ACCEPTED |
| **0072** | **MvccSqlEngine BACE open (G21)** | **ACCEPTED** |

**All four SQL engines** — `SqlEngine`, `DomainSqlEngine`,
`HistorySqlEngine`, `MvccSqlEngine` — **now accept BACE
wiring.** The operator-facing BACE surface for Bradian's
relational stack is complete.

## Remaining BACE workstream candidates

- **G23 DDL coordinator BACE**
  (`enable_epoch_row_writes`, `execute_ddl`,
  `activate_schema`). Would unblock the observed scope
  limit from ADR 0070 §6 item 6 and the related deferrals
  in ADRs 0071, 0072.
- **Authenticated envelope (`BACA`).** Different family of
  work; adds integrity authentication to the envelope.
- **`.brcs` carrier BACE.** Analytical column segments
  under BACE wrapping. Independent carrier.
- **BRWL carrier BACE.** WAL frame compression. Independent
  carrier.
- **Dictionary scope.** BACE §4.2
  dictionary-mode compression for shared-prefix columns.

## Baseline + commit

Baseline: `f05188a` (ADR 0072 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0072](../adr/0072-mvcc-sql-bace-open.md).
