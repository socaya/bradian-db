# BACE backup carrier adapter verification (ADR 0067)

14 November 2026. Baseline: `a8d2599`. First BACE **carrier**
adapter after the ADR 0065 row-store integration. Closes a real
regression hole: backup of a non-Raw BACE-opt-in store now
verifies, publishes, and restores through a new
`restore_backup_with_bace` entry point.

## Executed result

**950 Rust tests passed, 0 failed, 0 ignored.** Clippy clean on
`bradian-relational --all-targets`.

| Executed check | Result |
|---|---|
| `cargo check -p bradian-relational` | Passed |
| `cargo test --workspace --locked` | 950 passed, 0 failed |
| `cargo test -p bradian-relational --test backup_restore` | 23 passed (13 existing + 10 new) |
| `cargo clippy -p bradian-relational --all-targets --locked -- -D warnings` | Passed |

## What landed

### `crates/bradian-relational/src/backup.rs`

Additive on-disk change. No `BRBKv001` → `BRBKv002` bump.

```rust
pub struct BackupFlags {
    pub payload_has_bace_pages: bool,
}

pub struct BackupReceipt {
    pub checkpoint_lsn: u64,
    pub last_lsn: u64,
    pub rows: u64,
    pub flags: BackupFlags,         // NEW — at bytes 32..36 (u32 LE)
}
```

Descriptor decode:

- Legacy backups (bytes 32..36 = `0x00000000`) decode to `flags =
  default()`. CRC32 already covered those zeros, so legacy bytes
  decode unchanged.
- Any reserved bit non-zero fails closed with
  `Corruption("reserved backup flag bits non-zero")`.

`create_backup` and `verify_directory` gain a new
`bace: Option<&BaceWiring>` parameter. Verification routes through
`RowStore::open_with_bace` when wiring is present (either because
the receipt flag is set or because the caller volunteered wiring
on a legacy-shape backup — ADR 0065 §3.5 tolerates that).

New public API:

```rust
pub fn restore_backup_with_bace(
    backup: &Path,
    destination: &Path,
    policies: StorageLimits,
    wiring: BaceWiring,
) -> StoreResult<BackupReceipt>;
```

Legacy `restore_backup` / `restore_backup_with_policies` now
fail closed with
`StoreError::BackupRestoreRequiresBaceWiring` **before** creating
any staging directory when the receipt declares BACE pages.

### `crates/bradian-relational/src/row_store.rs`

```rust
pub fn backup_current_generation(&self, destination: impl AsRef<Path>) -> StoreResult<BackupReceipt>
```

now derives the flag:

```rust
let has_bace_pages = self.bace_wiring.as_ref()
    .map(|w| resolve_bace_codec(Some(w)).ok().flatten().is_some())
    .unwrap_or(false);
```

Rule: flag set iff wiring is present AND the policy resolves to a
non-Raw codec. A Raw-profile BACE-opt-in store has wiring but
emits v1 pages — the flag stays clear and the backup is
restorable through every entry point. The wiring is also passed
through to `backup::create_backup` so staging-side verification
can decode v2 pages.

`backup_to` is unchanged (still `self.checkpoint()?; self.backup_current_generation(destination)`).

New facade:

```rust
pub fn restore_backup_with_bace(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    policies: StorageLimits,
    wiring: BaceWiring,
) -> StoreResult<BackupReceipt>;
```

New error:

```rust
#[error("backup carries BACE v2 pages; restore_backup_with_bace is required")]
BackupRestoreRequiresBaceWiring,
```

`decode_data_page_payload` promoted from `fn` to `pub(crate) fn`
so `index_recovery` can reuse it.

### `crates/bradian-relational/src/index_recovery.rs` — **scope expansion**

Discovered during implementation that the draft §3.5 claim
"`verify_checkpoint_index` stays unchanged" was wrong:
`verify_with_limits` → `scan_snapshot` read `page.payload`
directly and hit `Corruption("snapshot row record bounds
invalid")` on BACE v2 pages.

Changes:

- `scan_snapshot(root, lsn, limits, bace: Option<&BaceWiring>)` —
  now routes each data page through `decode_data_page_payload`.
  Row pointers continue to reference the DECODED payload offset
  (matches the write-side pointer semantics from ADR 0065 R.2).
- New public `verify_with_limits_bace(root, lsn, limits, bace)`.
  The non-`_bace` `verify_with_limits` keeps its signature and
  forwards to `None`.
- New `pub(crate) rebuild_missing_with_limits_bace(...)`. The
  non-`_bace` wrapper was removed (dead code after thread-through).
- All three `RowStore` call sites (`verify_checkpoint_index`,
  `rebuild_missing_checkpoint_index`, the open-time
  `rebuild_missing_with_limits` invocation inside `open_inner`,
  and the pre-manifest-checkpoint upgrade path inside `checkpoint`)
  thread `self.bace_wiring.as_ref()` through.

### 10 new tests in `tests/backup_restore.rs`

| # | Test | ADR §3.7 item |
|---|---|---|
| 1 | `backup_receipt_roundtrip_with_flag_clear_yields_canonical_zero_flag_bytes` | 1 |
| 2 | `backup_receipt_roundtrip_with_flag_set_yields_bit_zero_at_byte_32` | 2 |
| 3 | `backup_receipt_decode_rejects_non_zero_reserved_flag_bits` | 3 |
| 4 | `backup_receipt_decode_rejects_flag_byte_tamper_without_crc_update` | 4 |
| 5 | `bace_raw_profile_backup_has_flag_clear_and_restores_through_legacy_path` | 5 |
| 6 | `bace_balanced_backup_has_flag_set_and_legacy_restore_fails_closed` | 6 |
| 7 | `bace_balanced_backup_restores_through_with_bace_entry_point` | 7 |
| 8 | `bace_backup_restore_fails_closed_when_wiring_lacks_source_codec` | 8 |
| 9 | `legacy_pre_bace_backup_restores_through_with_bace_entry_point` | 9 |
| 10 | `legacy_restore_fails_closed_on_tampered_flag_set_backup_before_touching_destination` | 10 (revised) |

**Item 10 revised.** The draft §3.7 item 10 named "source-side
verification still rejects a BACE-opt-in backup created via the
legacy `backup_to` path." That path is no longer reachable
through the public API: `backup_current_generation` always
threads `self.bace_wiring` through. The shipped test instead
exercises the still-observable fail-closed path: a receipt whose
flag bytes have been doctored to claim BACE pages (with a valid
CRC) must be rejected by the legacy restore entry point **before
any staging directory is created**.

## Compliance against ADR 0067

| ADR §3.7 item | Status |
|---|---|
| 1 flag-clear round-trip canonical | ✓ |
| 2 flag-set bit layout | ✓ |
| 3 reserved bits strict | ✓ |
| 4 CRC covers flag bytes | ✓ |
| 5 Raw-profile flag-clear + legacy restore works | ✓ |
| 6 Balanced-profile flag-set + legacy restore fails closed pre-dest | ✓ |
| 7 Balanced round-trip through `_with_bace` | ✓ |
| 8 missing-codec recovery fails closed | ✓ |
| 9 legacy backup through `_with_bace` succeeds | ✓ |
| 10 unreachable-from-public-API; revised to observable path | ✓ (revised) |

## What this ADR explicitly does NOT do (unchanged)

- Touch `database_backup.rs` or `history_backup.rs`.
- Change the on-disk file-copy set.
- Introduce archive-level compression or encryption.
- Change the `BRBKv001` descriptor magic.
- Add new policy keys.

## Known gap surfaced for a follow-up ADR

`database_backup.rs` (coordinator over three separate `backup.rs`
archives — catalog/domain/lifecycle) and `history_backup.rs`
(G22/G23 history-domain archives) now carry the same gap that
this ADR closed for `backup.rs`. They continue to work for Raw-
profile BACE-opt-in stores (no v2 pages emitted) and for legacy
non-BACE stores. A non-Raw BACE-opt-in coordinator backup will
fail verification until a sibling ADR extends them.

## Baseline + commit

Baseline: `a8d2599` (ADR 0067 PROPOSED).
Implementation commit: assigned by this commit.

See [ADR 0067](../adr/0067-backup-bace-carrier-adapter.md).
