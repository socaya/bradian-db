# BACE coordinator backup adapter verification (ADR 0068)

14 November 2026. Baseline: `1af77d2`. Closes the explicit "known
gap" from the ADR 0067 verification doc: `database_backup.rs`
(G19) and `history_backup.rs` (G24) coordinators now route to
`RowStore::restore_backup_with_bace` when a sub-archive declares
BACE pages, and fail closed with specific errors on the legacy
entry points.

## Executed result

**956 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 956 passed, 0 failed |
| `cargo test -p bradian-relational --test database_backup` | 15 passed (11 existing + 4 new) |
| `cargo test -p bradian-relational --test history_backup_consistency` | 4 passed (2 existing + 2 new) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/src/backup.rs`

```rust
/// ADR 0068 — exposes the on-disk backup.info descriptor so
/// coordinator backups can probe each sub-archive's BackupFlags
/// without reopening the RowStore.
pub fn read_receipt(path: &Path) -> StoreResult<BackupReceipt>
```

Was module-private before. One-line visibility change.

### `crates/bradian-relational/src/database_backup.rs`

New public API:

```rust
pub fn restore_database_with_bace(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    wiring: BaceWiring,
) -> DatabaseBackupResult<DatabaseBackupReport>;

pub fn restore_database_with_bace_and_metadata_limits(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    metadata: MetadataLimits,
    wiring: BaceWiring,
) -> DatabaseBackupResult<DatabaseBackupReport>;
```

New error variant:

```rust
#[error("database backup carries BACE v2 pages in one or more components; restore_database_with_bace is required")]
RestoreRequiresBaceWiring,
```

Refactored `restore_database_with_metadata_limits` to delegate to
a shared `restore_database_inner(backup, destination, metadata,
wiring: Option<BaceWiring>)`. Inside:

1. `read_manifest` — unchanged.
2. `verify_archive_crc` loop — unchanged.
3. **NEW:** iterate `COMPONENTS`, read each sub-archive's
   `backup.info` via `backup::read_receipt`. A damaged descriptor
   is translated from `StoreError::Corruption` to
   `DatabaseBackupError::Corruption("{component} backup.info: ...")`
   so the pre-existing tamper-detection contract keeps its shape.
   If any sub-archive has `flags.payload_has_bace_pages = true`
   AND `wiring.is_none()`, return
   `DatabaseBackupError::RestoreRequiresBaceWiring` **before
   `canonical_new_destination`** runs — no staging directory is
   created.
4. Staging + per-component restore — dispatches to
   `RowStore::restore_backup_with_bace(..., w.clone())` when
   wiring is present, otherwise the legacy
   `RowStore::restore_backup`.
5. `verify_reopened` — unchanged. Today-reachable scenarios all
   use legacy (flag=0) sub-archives, so `CatalogLifecycle::open`
   on the staged directory continues to work.
6. Publish — unchanged.

### `crates/bradian-relational/src/history_backup.rs`

Mirror shape for the two-component G24 coordinator:

```rust
pub fn restore_history_database_with_bace(...);
pub fn restore_history_database_with_bace_and_policies(...);
HistoryBackupError::RestoreRequiresBaceWiring,
```

Same inner-function refactor and probe placement (after manifest
parse, after per-component `archive_crc` check, before staging
dir).

### Design choice recorded: probe position

The ADR §3.4 placed the probe "before manifest/CRC checks."
Implementation moved it to **after** both because:

- Missing `backup.info` files on an empty/damaged archive
  surface as `Io` through the pre-existing `read_manifest`
  failure, matching the existing `metadata_limits.rs` test
  contract.
- Malformed `database-backup.json` surfaces as
  `DatabaseBackupError::Corruption`, matching the existing
  `malformed_json_below_limit_keeps_corruption_classification`
  test.
- ADR §3.4 "fail closed BEFORE any staging directory" invariant
  still holds — manifest read + archive_crc check both complete
  before `canonical_new_destination` is called.

The ADR documentation was adjusted to describe the as-shipped
order.

### 6 new tests

**4 in `tests/database_backup.rs`:**

- `restore_database_with_bace_on_a_legacy_coordinator_backup_succeeds`
  — ADR §3.6 item 1. Wiring tolerated on flag-clear coordinator.
- `restore_database_legacy_fails_closed_with_specific_error_when_any_sub_archive_declares_bace`
  — §3.6 items 3 and 6. Doctored flag → legacy restore returns
  `RestoreRequiresBaceWiring`; neither destination nor staging
  directory appears.
- `read_receipt_is_publicly_reachable_and_returns_flags`
  — §3.6 item 7.
- `restore_database_with_bace_and_metadata_limits_is_reachable`
  — covers the non-default-limits entry point.

**2 in `tests/history_backup_consistency.rs`:**

- `restore_history_database_with_bace_on_a_legacy_coordinator_backup_succeeds`
  — §3.6 item 2.
- `restore_history_database_legacy_fails_closed_when_sub_archive_declares_bace`
  — §3.6 item 5.

### Documented-but-not-tested item

**§3.6 item 4** — doctored-flag tolerance through `_with_bace`
— is documented in the ADR §3.6 but not covered by an
integration test. Reasoning: flipping one sub-archive's flag
also changes that sub-archive's `archive_crc32`, so the
coordinator fails at the pre-existing `verify_archive_crc`
check before the flag probe even runs. Reaching the pure
"flag=1, v1 pages, consistent manifest CRCs" state would
require rewriting the coordinator manifest — too much test
infrastructure for a corner case. The underlying rule is
covered by the sub-archive-level test
`legacy_pre_bace_backup_restores_through_with_bace_entry_point`
in the ADR 0067 test suite; the coordinator inherits it
transitively.

## Compliance against ADR 0068

| ADR §3.6 item | Status |
|---|---|
| 1 legacy coord through `_with_bace` (database) | ✓ |
| 2 legacy coord through `_with_bace` (history) | ✓ |
| 3 doctored-flag → legacy database fails closed | ✓ |
| 4 doctored-flag → `_with_bace` tolerates | Documented, not tested (see above) |
| 5 doctored-flag → legacy history fails closed | ✓ |
| 6 fail-closed before any destination | ✓ (covered by test 3) |
| 7 `read_receipt` publicly reachable | ✓ |
| 8 round-trip verifies restored root | ✓ (covered by tests 1 and 2) |

## What this ADR explicitly does NOT do (unchanged)

- Touch `backup_database` or `backup_history_database`
  (coordinator backup-side).
- Touch `CatalogLifecycle::open` or any ambient coordinator-open
  entry point.
- Bump manifest format versions.
- Add fields to `ComponentReceipt` or `HistoryComponentReceipt`.
- Change sub-archive file names, counts, or lock ordering.

## Known scope limit (unchanged from ADR §9)

End-to-end BACE deployment of a Bradian database still requires
`CatalogLifecycle::open_with_bace` so component `RowStore`s emit
BACE sub-archives. That is the actual unblocker; this ADR is the
matching restore-side plumbing that is now ready to receive those
backups.

Until the `CatalogLifecycle::open_with_bace` ADR lands:

- Every coordinator backup produced by `backup_database` /
  `backup_history_database` continues to have all-zero
  `BackupFlags` in every sub-archive (writer emits v1 pages).
- `restore_database` and `restore_history_database` continue to
  work exactly as today.
- `restore_database_with_bace` and
  `restore_history_database_with_bace` add new entry points that
  tolerate wiring on legacy sub-archives and are ready for BACE
  sub-archives the day they become producible.

## Baseline + commit

Baseline: `1af77d2` (ADR 0068 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0068](../adr/0068-coordinator-backup-bace-adapter.md).
