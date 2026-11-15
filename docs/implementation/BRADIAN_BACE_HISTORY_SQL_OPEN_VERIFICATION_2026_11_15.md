# BACE HistorySqlEngine open verification (ADR 0070)

15 November 2026. Baseline: `d66d063`. G24 writer-side mirror
of ADR 0069. Threads BACE wiring through the independent
durable-history stack (`DurableMvccStore` →
`DurableTypedStore` → `HistorySqlEngine`) so a G24 database
backup produced via BACE-opened engine carries
`flags.payload_has_bace_pages = true` on both sub-archives.

## Executed result

**965 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 965 passed, 0 failed |
| `cargo test -p bradian-relational --test history_backup_consistency` | 6 passed (2 pre-existing + 4 new) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/src/durable_mvcc.rs`

```rust
impl DurableMvccStore {
    pub fn open_with_bace(path, wiring) -> DurableMvccResult<Self>;
    pub fn open_with_bace_and_resource_limits(path, wiring, limits, transactions) -> DurableMvccResult<Self>;
    fn open_inner(path, limits, transactions, wiring: Option<BaceWiring>);
}
```

Shared body delegates to `RowStore::open_with_bace` when wiring
is present, `RowStore::open` otherwise. Format-marker bootstrap
unchanged.

### `crates/bradian-relational/src/durable_typed.rs`

```rust
impl DurableTypedStore {
    pub fn open_with_bace(root, tables, wiring);
    pub fn open_with_bace_and_history_limits(root, tables, wiring, limits);
    pub fn open_with_bace_and_resource_limits(root, tables, wiring, limits, transactions);
    pub(crate) fn open_coordinated_with_bace(root, catalog, wiring, limits, transactions);
}
```

Refactored the coordinated path into a shared
`finish_open_coordinated(store, catalog)` helper so the BACE
and non-BACE coordinated opens share identical post-store
verification logic.

### `crates/bradian-relational/src/history_sql.rs`

```rust
pub struct HistorySqlEngine {
    // ... existing fields ...
    bace_wiring: Option<BaceWiring>,          // NEW
}

impl HistorySqlEngine {
    pub fn open_with_bace(root, wiring);
    pub fn open_with_bace_and_history_limits(root, wiring, limits);
    pub fn open_with_bace_and_resource_limits(root, wiring, limits, transactions);
    pub fn backup_to_with_bace(&self, destination);
    fn open_inner(root, limits, transactions, wiring: Option<BaceWiring>);
}
```

`open_inner` constructs the two component stores via the matching
`_with_bace` paths when wiring is present. `bace_wiring` is
stored on the engine so `backup_to_with_metadata_limits` can
pass it to `history_backup::backup_history_database`.

### `crates/bradian-relational/src/history_backup.rs` — scope expansion

Matches the ADR 0069 scope expansion for G19:

- `backup_history_database` gained `bace: Option<&BaceWiring>`.
- Staging-side sub-archive restores dispatch to
  `RowStore::restore_backup_with_bace` under BACE.
- `verify_root` gained `bace: Option<&BaceWiring>` and reopens
  each component via `PersistentCatalog::open_with_bace` /
  `DurableTypedStore::open_with_bace_and_history_limits` when
  wiring is present.
- The restore-side `restore_history_database_inner` already
  owned a `wiring` (from ADR 0068); it now also threads it
  through `verify_root`.
- `#[allow(clippy::too_many_arguments)]` on
  `backup_history_database` (its 6-arg signature now has 1
  more).

### 4 new tests in `tests/history_backup_consistency.rs`

| # | Test | ADR §3.6 item |
|---|---|---|
| 1 | `history_sql_engine_open_with_bace_round_trips_a_legacy_migrated_root` | 1, 2, 3 (SELECT works) |
| 2 | `backup_to_with_bace_emits_flag_set_sub_archives_for_history_coordinator` | 4, 8 (legacy `backup_to` on `_with_bace`-opened engine also produces BACE sub-archives per ADR 0069 §5 Q3) |
| 3 | `bace_history_backup_round_trips_through_restore_history_database_with_bace` | 5 (end-to-end G24 BACE loop closes) |
| 4 | `bace_flag_is_preserved_across_history_coordinator_round_trip` | 6 adjusted (see §6 Q6 limitation below) |

### Observed scope limit on §3.6 item 6

The drafted §3.6 item 6 (legacy `HistorySqlEngine::open` on a
BACE-written restored destination fails closed) is not
reachable with the current G24 fixtures. The migrated G24 root
is checkpointed once during migration at a fixed LSN. Opening
with `_with_bace` and immediately calling `backup_to` reuses
the existing v1 snapshot (per the ADR 0065 "checkpoint is
idempotent at current LSN" rule in `RowStore::checkpoint_with_limits`),
so no v2 pages are produced.

Producing v2 pages through G24 would require either:
- `enable_epoch_row_writes` + `execute_ddl` DDL coordinator
  mutations (explicitly out of scope per ADR 0070 §5 Q2);
- `with_session` row mutations that also require epoch rows
  enabled, which requires the DDL coordinator.

Test 4 (`bace_flag_is_preserved_across_history_coordinator_round_trip`)
asserts what IS reachable today: the BackupFlags round-trip
through backup → restore so the plumbing is ready when a
future ADR enables BACE-aware DDL/epoch-row writes. The
fail-closed invariant for v2 pages in a
`HistorySqlEngine::open` call is proven at the RowStore layer
by ADR 0067's `backup_restore.rs` suite; the G24 coordinator
inherits the behavior transitively.

## Compliance against ADR 0070 §3.6

| Item | Status |
|---|---|
| 1 `DurableMvccStore::open_with_bace` round-trips | ✓ (covered by test 1 + end-to-end) |
| 2 `DurableTypedStore::open_with_bace_and_history_limits` round-trips | ✓ (covered by test 1 + end-to-end) |
| 3 `HistorySqlEngine::open_with_bace` + SELECT | ✓ test 1 |
| 4 `backup_to_with_bace` emits flag-set sub-archives | ✓ test 2 |
| 5 end-to-end BACE round-trip | ✓ test 3 |
| 6 Legacy open on BACE-written root fails closed | Partially — plumbing proven; actual v2 pages not reachable from today's G24 fixture; see §"Observed scope limit" |
| 7 Legacy restore fails closed on BACE sub-archives | ✓ (ADR 0068 test suite covers this path; backward-compatible) |
| 8 Legacy `backup_to` on `_with_bace`-opened engine also produces BACE | ✓ test 2 |

## What this ADR explicitly does NOT do (unchanged)

- Touch any on-disk format.
- Touch `DomainSqlEngine` or `SqlEngine` open paths.
- Touch G23 DDL coordinator / epoch-row-write flows.
- Add new policy keys or codecs.
- Introduce archive-level compression, encryption, or
  authentication.
- Change manifest version.

## Known scope limits recorded for follow-up ADRs

- **G23 DDL coordinator BACE** (`enable_epoch_row_writes`,
  `execute_ddl`, `activate_schema`). Follow-up ADR. Unblocks the
  §3.6 item 6 fail-closed scenario in a G24 context by producing
  actual v2 pages after schema mutations.
- **`DomainSqlEngine::open_with_bace`,
  `SqlEngine::open_with_bace`.** Thin derivatives of
  `MultiTableStore::open_with_bace` (ADR 0069) and
  `PersistentCatalog::open_with_bace` (ADR 0069). Follow-up ADR
  to expose the SQL-engine-facing BACE opens.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0063 | Prerequisites | ACCEPTED |
| 0064 | BRPG v2 page format | ACCEPTED |
| 0065 | RowStore checkpoint integration | ACCEPTED |
| 0066 | ZSTD codec | ACCEPTED |
| 0067 | Backup carrier adapter | ACCEPTED |
| 0068 | Coordinator backup carrier adapter (restore) | ACCEPTED |
| 0069 | CatalogLifecycle BACE open (G19 writer) | ACCEPTED |
| **0070** | **HistorySqlEngine BACE open (G24 writer)** | **ACCEPTED** |

Both coordinator families (G19 and G24) are now wired end-to-end
for BACE deployment at the RowStore layer. SQL-engine-facing
BACE opens and DDL coordinator BACE opens are the remaining
follow-up ADRs.

## Baseline + commit

Baseline: `d66d063` (ADR 0070 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0070](../adr/0070-history-sql-bace-open.md).
