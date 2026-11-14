# BACE CatalogLifecycle open verification (ADR 0069)

14 November 2026. Baseline: `a195e11`. Writer-side unblocker
that closes the BACE end-to-end loop for the G18/G19 coordinator:
open a Bradian database with BACE wiring, back it up, restore
it through ADR 0068 `restore_database_with_bace`, and reopen
with BACE wiring — every row recovered.

## Executed result

**961 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 961 passed, 0 failed |
| `cargo test -p bradian-relational --test database_backup` | 19 passed (14 pre-existing + 5 new) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/src/transaction_domain.rs`

```rust
impl MultiTableStore {
    pub fn open_with_bace(root, wiring: BaceWiring) -> DomainResult<Self>;
    pub fn open_with_bace_and_limits(root, wiring, limits) -> DomainResult<Self>;
    fn open_inner(root, limits, wiring: Option<BaceWiring>) -> DomainResult<Self>;
}
```

`open` and `open_with_limits` now delegate to `open_inner` with
`wiring = None`. The schema-replay and row-validation logic
after the storage open is unchanged.

### `crates/bradian-relational/src/persistent_catalog.rs`

Symmetric:

```rust
impl PersistentCatalog {
    pub fn open_with_bace(path, wiring) -> PersistentCatalogResult<Self>;
    pub fn open_with_bace_and_limits(path, wiring, limits) -> PersistentCatalogResult<Self>;
    fn open_inner(path, limits, wiring: Option<BaceWiring>);
}
```

### `crates/bradian-relational/src/lifecycle.rs`

```rust
pub struct CatalogLifecycle {
    // ... existing fields ...
    bace_wiring: Option<BaceWiring>,     // NEW — shared with sub-stores
    operation: Mutex<()>,
}

impl CatalogLifecycle {
    pub fn open_with_bace(root, wiring: BaceWiring) -> LifecycleResult<Self>;
    pub fn backup_database_to_with_bace(&self, destination)
        -> Result<DatabaseBackupReport, DatabaseBackupError>;
    fn open_inner(root, wiring: Option<BaceWiring>) -> LifecycleResult<Self>;
}
```

`open_inner` constructs the three component stores with a
single cloned wiring, preserves the existing marker/reconcile
logic, and stores `bace_wiring` on the coordinator so
`backup_database_to` can hand it to `backup_database`.

### `crates/bradian-relational/src/database_backup.rs` — scope expansion

**Not in the drafted ADR §3.4**, discovered during
implementation. The `backup_database` function (called by
`CatalogLifecycle::backup_database_to_*`) internally:

1. Backs up each component to a staging directory — ✓ already
   threads wiring via each store's `backup_current_generation`
   (ADR 0067 §3.3).
2. **Restores each sub-archive into `verified-root/` for
   staging-side verification** — uses `RowStore::restore_backup`
   (legacy), which ADR 0067 made fail closed on BACE
   sub-archives.
3. **Calls `verify_reopened`** which opens the staged root
   via `CatalogLifecycle::open` (legacy), which now hits v2
   pages and fails closed per ADR 0065 §3.4.

Both (2) and (3) had to be routed through the `_with_bace`
variants when wiring is present. Changes:

- `backup_database` gained `bace: Option<&BaceWiring>` arg.
- Staging-side sub-archive restores dispatch to
  `RowStore::restore_backup_with_bace` under BACE.
- `verify_reopened` gained `bace: Option<&BaceWiring>` arg,
  dispatches to `CatalogLifecycle::open_with_bace` under BACE.
- The restore-side call site (`restore_database_inner`) threads
  its own `wiring.as_ref()` through.
- `CatalogLifecycle::backup_database_to_with_metadata_limits`
  threads `self.bace_wiring.as_ref()` through.

The ADR was updated to reflect this scope.

### 5 new tests in `tests/database_backup.rs`

| # | Test | ADR §3.6 item |
|---|---|---|
| 1 | `catalog_lifecycle_open_with_bace_round_trips_a_legacy_source_directory` | 1, 2, 3 |
| 2 | `backup_database_through_bace_opened_lifecycle_emits_flag_set_sub_archives` | 5, 8 |
| 3 | `bace_coordinator_backup_round_trips_through_restore_database_with_bace` | 6 (end-to-end loop closes) |
| 4 | `legacy_catalog_lifecycle_open_on_a_bace_written_directory_fails_closed` | 7 |
| 5 | `bace_opened_lifecycle_backup_restorable_via_restore_database_with_bace` | 9 (confirms legacy coordinator entry on BACE lifecycle ALSO produces BACE sub-archives per ADR §3.4 / §5 Q3) |

**Test 4 error-path refinement.** The error surfaces as
`LifecycleError::Catalog(PersistentCatalogError::Storage(StoreError::BaceOpenWithoutBaceWiring))`
because the catalog component is opened first. The test matches
any of the three component-specific error variants carrying the
BACE-origin error string, since whichever component is opened
first in the trio drives the failure.

**Test 5 explicit behavior**: `CatalogLifecycle::backup_database_to`
(the LEGACY entry point) called on a `CatalogLifecycle::open_with_bace`
coordinator PRODUCES BACE sub-archives. This is the ADR §5 Q3
honesty-flag made concrete: `self.bace_wiring` on each sub-store
determines the on-disk format, independent of which coordinator
entry point the operator calls. `backup_database_to_with_bace`
is kept for API clarity (§5 Q3a).

## Compliance against ADR 0069 §3.6

| Item | Status |
|---|---|
| 1 `MultiTableStore::open_with_bace` round-trips | ✓ (covered by test 1 + end-to-end) |
| 2 `PersistentCatalog::open_with_bace` round-trips | ✓ (covered by test 1 + end-to-end) |
| 3 `CatalogLifecycle::open_with_bace` fresh | ✓ test 1 |
| 4 SQL round-trip | Deferred per §5 Q2; coordinator-level round-trip in test 3 |
| 5 BACE sub-archives emitted on flag-set | ✓ test 2 |
| 6 ADR 0068 restore closes the loop | ✓ test 3 |
| 7 Legacy open on BACE directory fails closed | ✓ test 4 |
| 8 Legacy `backup_database_to` on BACE-opened lifecycle also produces BACE | ✓ test 5 |
| 9 Legacy coordinator restore via `_with_bace` works on BACE sub-archives | ✓ test 5 |

## What this ADR explicitly does NOT do (unchanged)

- Touch any on-disk format.
- Add new policy keys, new codecs, new envelope families.
- Introduce archive-level compression, encryption, or
  authentication.
- Touch `HistorySqlEngine` or `history_backup.rs` writer.
- Touch `DomainSqlEngine` or `SqlEngine` open paths.
- Change the operation-mutex / lock ordering discipline.

## Known scope limits recorded for follow-up ADRs

- **`HistorySqlEngine::open_with_bace`.** G24 coordinator still
  routes through legacy opens. ADR 0070 candidate.
- **`DomainSqlEngine::open_with_bace`.** Thin derivative of
  `MultiTableStore::open_with_bace` landing in this ADR.
  Follow-up ADR to add the SQL-engine-facing entry point.
- **`SqlEngine::open_with_bace`.** Likewise.
- **Policy service identity in the lifecycle marker row.** Draft
  Q5 answered NO; a reopen with mismatched wiring surfaces as
  `BaceCodecUnavailable` at the deep-fail path, which is
  specific and actionable enough.

## BACE workstream progress (as of this ADR)

| ADR | Component | Status |
|---|---|---|
| 0060–0063 | Prerequisites | ACCEPTED |
| 0064 | BRPG v2 page format | ACCEPTED |
| 0065 | RowStore checkpoint integration | ACCEPTED |
| 0066 | ZSTD codec | ACCEPTED |
| 0067 | Backup carrier adapter | ACCEPTED |
| 0068 | Coordinator backup carrier adapter (restore) | ACCEPTED |
| **0069** | **CatalogLifecycle BACE open (writer)** | **ACCEPTED** |

End-to-end BACE deployment of a G19 Bradian database is now
shippable: operator opens with `CatalogLifecycle::open_with_bace`,
backs up with `backup_database_to`, restores with
`restore_database_with_bace`, reopens with
`CatalogLifecycle::open_with_bace`.

## Baseline + commit

Baseline: `a195e11` (ADR 0069 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0069](../adr/0069-catalog-lifecycle-bace-open.md).
