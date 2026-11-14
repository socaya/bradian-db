# ADR 0068 — Coordinator backup BACE adapter (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE prerequisite stack (0060–0063), the
accepted row-store integration (0065), and the accepted backup
carrier adapter (0067). Closes the "known gap" §9 of the ADR 0067
verification doc: the two coordinator backup carriers
(`database_backup.rs`, `history_backup.rs`) still call the legacy
`RowStore::restore_backup`, which now fails closed on sub-archives
that carry BACE v2 pages.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- `database_backup.rs` (G19) orchestrates three independent
  `backup.rs` sub-archives (catalog / shared-domain / lifecycle)
  behind one JSON manifest `BRADIAN-DATABASE-BACKUP-V1`. Restore
  invokes `RowStore::restore_backup` per component.
- `history_backup.rs` (G24) orchestrates two independent
  sub-archives (catalog / history-domain) behind one JSON
  manifest `BRADIAN-HISTORY-BACKUP-V1`. Restore invokes
  `RowStore::restore_backup` per component.
- After ADR 0067, `RowStore::restore_backup` fails closed with
  `BackupRestoreRequiresBaceWiring` whenever the sub-archive's
  own `backup.info` has `payload_has_bace_pages = true`.
  Coordinator restore inherits that error — but the coordinator
  has no entry point to supply BACE wiring, so operators have
  no way to restore a BACE-wrapped coordinator backup.
- **How does a BACE-wrapped coordinator backup come into
  existence?** Today, it doesn't: `CatalogLifecycle::open` opens
  its three component `RowStore`s via the legacy
  `RowStore::open`, which emits only v1 pages. A BACE-opt-in
  coordinator requires `CatalogLifecycle::open_with_bace` — out
  of scope for this ADR. This ADR is therefore partly
  **future-proofing**: the coordinator restore path becomes
  BACE-aware now, so when CatalogLifecycle acquires BACE opens
  in a follow-up ADR, the restore path is already in place.
- The ADR is also **today-reachable** via the honest fail-closed
  test case: an operator who hand-constructs a coordinator-shape
  backup where one sub-archive is BACE-wrapped can be refused
  by the legacy restore with a specific error instead of a
  staging-side verification mystery.

## 2. Scope of this ADR

**In scope:**

- **Restore-side only.** `restore_database` and
  `restore_history_database` learn to probe each sub-archive's
  `backup.info` for the ADR 0067 flag and route through
  `RowStore::restore_backup_with_bace` when set.
- Two new public entry points:
  - `database_backup::restore_database_with_bace(backup, destination, metadata, wiring)`
  - `history_backup::restore_history_database_with_bace(backup, destination, metadata, wiring)`
- Legacy entry points (`restore_database`,
  `restore_history_database`) fail closed BEFORE creating any
  staging directory when any sub-archive declares BACE pages.
  Specific errors, not generic corruption.
- Two new error variants, one per coordinator:
  - `DatabaseBackupError::RestoreRequiresBaceWiring`
  - `HistoryBackupError::RestoreRequiresBaceWiring`
- Shared-wiring model: one `BaceWiring` applies to every
  component in the coordinator. The three (or two) sub-archives
  share policy and registry. A follow-up may add per-component
  wirings if operationally required; the first implementation
  keeps the API bounded.
- Legacy sub-archives (flag=0) within a coordinator-shape backup
  restore successfully through both the legacy and `_with_bace`
  entry points.
- Mixed sub-archives: some with the flag set, some without. The
  coordinator restore iterates sub-archives and dispatches per-
  component.
- Tests covering the §3.6 checklist below.

**Explicitly out of scope** (each needs its own ADR or is a
separate follow-up):

- **Coordinator backup-side BACE wiring.** `backup_database`
  and `backup_history_database` are unchanged. They still accept
  component `RowStore`s and `PersistentCatalog`s whose backing
  storage emits v1 pages — the only shape producible today.
- **`CatalogLifecycle::open_with_bace`.** A future ADR adds
  that; it is the actual unblocker that will cause coordinator
  backups to emit BACE sub-archives.
- **Manifest version bump** (`BRADIAN-DATABASE-BACKUP-V1` →
  `-V2`). Not needed: the ADR 0067 flag lives in each
  sub-archive's `backup.info`, which the manifest already
  covers via `archive_crc32`. The coordinator manifest itself
  is blind to BACE.
- **`ComponentReceipt` / `HistoryComponentReceipt` schema
  changes.** Not needed either. The authority for whether a
  sub-archive has BACE pages is the sub-archive's own
  `backup.info`; adding a duplicate field would create a
  consistency obligation and offer no new truth.
- **Changing the number of sub-archives, their names, or
  their lock ordering.**
- **Authentication, encryption, remote storage.** ADR 0069
  reserved for authenticated envelope (`BACA`).
- **Concurrent/online coordinator backup.** The existing
  "hold the three locks, copy files, verify, publish" model
  stays intact.

## 3. Proposed design

### 3.1 Public API additions

```rust
// src/database_backup.rs
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

```rust
// src/history_backup.rs
pub fn restore_history_database_with_bace(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    wiring: BaceWiring,
) -> HistoryBackupResult<HistoryBackupReport>;

pub fn restore_history_database_with_bace_and_policies(
    backup: impl AsRef<Path>,
    destination: impl AsRef<Path>,
    policies: StorageLimits,
    history: HistoryLimits,
    wiring: BaceWiring,
) -> HistoryBackupResult<HistoryBackupReport>;
```

### 3.2 New error variants

```rust
// DatabaseBackupError
#[error("database backup carries BACE v2 pages in one or more components; restore_database_with_bace is required")]
RestoreRequiresBaceWiring,

// HistoryBackupError
#[error("history backup carries BACE v2 pages in one or more components; restore_history_database_with_bace is required")]
RestoreRequiresBaceWiring,
```

Fired BEFORE `canonical_new_destination` or any `fs::create_dir`.
No side effects on fail-closed.

### 3.3 Sub-archive flag probe

New helper in `backup.rs` (adjacent to the existing
`read_receipt`):

```rust
pub fn read_backup_info_flags(archive_dir: &Path) -> StoreResult<BackupFlags>;
```

Returns `BackupFlags { payload_has_bace_pages: false }` when the
file is absent AND the directory has no sub-archive — but the
coordinators already require the component directories exist,
so absence of `backup.info` is itself a `Corruption` error.

Actually the simpler shape is to expose `read_receipt` publicly
and let callers read `.flags.payload_has_bace_pages` off the
returned `BackupReceipt`. This avoids inventing a second getter.
Draft chooses this — expose `backup::read_receipt` (currently
module-private) as `pub`.

### 3.4 Coordinator restore routing

Inside `restore_database_inner(..., wiring: Option<BaceWiring>)`:

```rust
let flags_per_component: Vec<BackupFlags> = COMPONENTS.iter()
    .map(|c| backup::read_receipt(&backup.join(c).join("backup.info"))
        .map(|r| r.flags))
    .collect::<StoreResult<Vec<_>>>()?;

let any_bace = flags_per_component.iter().any(|f| f.payload_has_bace_pages);

if any_bace && wiring.is_none() {
    return Err(DatabaseBackupError::RestoreRequiresBaceWiring);
}

// ... existing manifest + archive_crc verification unchanged ...

for (component, expected, flags) in COMPONENTS.iter()
    .zip(sub_receipts).zip(flags_per_component.iter())
{
    let actual = match (&wiring, flags.payload_has_bace_pages) {
        (Some(w), _) => RowStore::restore_backup_with_bace(
            backup.join(component), staged.join(component),
            StorageLimits::default(), w.clone(),
        )?,
        (None, false) => RowStore::restore_backup(
            backup.join(component), staged.join(component),
        )?,
        (None, true) => unreachable!(), // guarded above
    };
    // ... receipt_matches check unchanged ...
}
```

The existing `verify_reopened` step opens the full
`CatalogLifecycle` on the staged directory. That function still
uses `CatalogLifecycle::open` which uses `RowStore::open` on
each component — so **a staged directory with v2 pages inside
a component RowStore would fail at this step**. This is
acceptable as a scope boundary: the coordinator restore
succeeds at placing the right bytes but the final "reopen the
whole thing and verify" step depends on CatalogLifecycle being
BACE-aware, which is a separate ADR. Today-reachable test
scenarios all use legacy (flag=0) sub-archives, so
`verify_reopened` works unchanged.

`history_backup.rs` mirrors the same structure, two components.

### 3.5 Backup side unchanged

`backup_database` and `backup_history_database` take their
component `RowStore`s / `MultiTableStore` / `PersistentCatalog`
exactly as they do today. Those stores have no BACE wiring yet
(ambient `CatalogLifecycle::open` opens them through
`RowStore::open`), so they emit v1 pages, the
`BackupReceipt.flags.payload_has_bace_pages` is `false`, and
the coordinator manifest carries no BACE indicator.

When a future ADR adds `CatalogLifecycle::open_with_bace`, the
coordinator backup-side plumbing will need to pass through the
wiring. That is out of scope here.

### 3.6 Compliance obligations (implementation test checklist)

An implementation of this ADR MUST demonstrate every item below:

1. `restore_database_with_bace` on a **legacy coordinator
   backup** (all three sub-archives flag=0) succeeds and
   reopens correctly. Wiring is tolerated on v1-only
   sub-archives (ADR 0065 §3.5 rule).
2. `restore_history_database_with_bace` on a legacy history
   coordinator backup (both sub-archives flag=0) succeeds.
3. A coordinator backup where **one** sub-archive is doctored
   to have `payload_has_bace_pages = true` (with the flag CRC
   recomputed so the sub-archive descriptor is self-consistent)
   fails `restore_database` with the new
   `RestoreRequiresBaceWiring` error, BEFORE any staging
   directory is created.
4. The same doctored-flag coordinator fails
   `restore_database_with_bace` as well — because the flag
   lies but the underlying pages are v1, the coordinator does
   not actually need wiring and the `_with_bace` path routes
   through `restore_backup_with_bace`, which per ADR 0065 §3.5
   tolerates wiring on v1 directories. So this path actually
   **succeeds**. Test asserts that specifically — the
   coordinator does not add its own consistency check beyond
   "need wiring if any sub-archive has flag set."
5. Symmetric tests for `history_backup.rs` items 3–4.
6. `restore_database` legacy entry fails closed with
   `RestoreRequiresBaceWiring` without touching the
   destination (no staging dir created, no sub-archive
   decoded).
7. `backup::read_receipt` is reachable from outside the crate
   and returns a `BackupReceipt` with the correct flag.
8. Round-trip of `restore_database_with_bace(backup, dest,
   wiring)` on a legacy coordinator backup followed by
   `restore_database(dest, ...)` on the republished
   destination (if destination becomes a new coordinator
   backup — it doesn't; `restore_database` publishes a
   database root, not another backup). Instead: assert the
   restored database root's tables and lifecycle state match
   what the original source produced.

## 4. Alternatives considered

**A. Add `payload_has_bace_pages` as a serde field to
`ComponentReceipt` / `HistoryComponentReceipt`.** Explicit at
the manifest level. Rejected: creates a duplicate source of
truth; the manifest's `archive_crc32` already transitively
covers the sub-archive's `backup.info`. Reading the flag from
the sub-archive avoids the consistency obligation.

**B. Bump manifest version to `BRADIAN-DATABASE-BACKUP-V2`.**
Clean separation. Rejected: no non-additive manifest change
is required. Per-sub-archive flag detection works without a
version bump.

**C. Take three separate `BaceWiring`s in the coordinator
API, one per component.** Future-flexible. Rejected for v1: the
three components share policy in practice; adding per-component
wiring widens the API without a validated use case.

**D. Auto-attach a default BACE wiring on legacy
`restore_database` when sub-archive flags demand it.** Simpler
operator UX. Rejected for the same reason ADR 0067 rejected
auto-attach: silent format assumptions would violate the
"explicit opt-in" culture.

**E. Scope this ADR to both backup and restore sides.** Needs
`CatalogLifecycle::open_with_bace` first. Rejected: too much
in one ADR. Restore-side only is a clean milestone.

**F. Delete the legacy `restore_database` entry point and only
keep `restore_database_with_bace`.** Simpler API. Rejected:
breaks every existing caller and operator script. Additive
new entry point is the honest path.

## 5. Open design questions (owner review)

1. **Entry-point naming.** Draft:
   `restore_database_with_bace` and
   `restore_history_database_with_bace`. Alternatives:
   `_including_bace`, `_bace`, overloading (impossible in
   Rust). Confirm.
2. **Shared single wiring vs per-component.** Draft: single
   `BaceWiring` applies to all three (G19) or both (G24)
   components. Confirm.
3. **`backup::read_receipt` visibility.** Draft: promote the
   existing private `read_receipt` to `pub fn`. Alternative:
   add a `BackupReceipt::from_path(..)` associated function
   that calls into it. Confirm.
4. **Expose `BackupFlags::from_u32` publicly?** Draft: NO, keep
   it crate-private; callers interact only via
   `BackupReceipt::decode` → `.flags`. Confirm.
5. **Test item 4 behaviour.** Draft: a doctored-flag-but-v1-
   pages sub-archive restores successfully through
   `_with_bace` — the coordinator does not add its own
   consistency audit beyond "need wiring if any flag set."
   Alternative: add a stronger "manifest consistency" check
   that rejects flag=1 sub-archives whose pages are actually
   v1. Rejected in draft: that is the sub-archive's own
   integrity obligation, not the coordinator's. Confirm draft.
6. **Should `verify_reopened` also become BACE-aware now?**
   Draft: NO. `verify_reopened` opens the full
   `CatalogLifecycle`, which has no BACE wiring yet; fixing
   that is a separate ADR. Today-reachable tests use legacy
   sub-archives so `verify_reopened` works unchanged. Confirm
   deferral.
7. **Should the coordinator **manifest** carry a top-level
   "any sub-archive uses BACE" flag for cheap pre-flight?**
   Draft: NO. Reading three (or two) `backup.info` files is
   cheap. Avoiding a manifest field preserves non-additive
   silence. Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change. Sub-archives
  continue to carry their own LSN-consistent state.
- **INV-002 one authoritative state:** no change. Restore
  still publishes only after full verification.
- **INV-003 no authorization gap:** no change. The BACE
  envelope carries no authorization context; CRCs and
  rename-after-verify discipline remain the guards.
- **INV-004 no unsafe automatic availability:** N/A (local).
- **INV-005 recoverable lifecycle:** additive. Legacy
  coordinator backups continue to restore through legacy
  entry points. The new `_with_bace` entry points tolerate
  legacy sub-archives too.

## 7. Dependencies this ADR would add

None. No new crate deps. Implementation confined to:

- `crates/bradian-relational/src/database_backup.rs` — add
  the `_with_bace` entry points, the new error variant, the
  routing logic. One `use crate::row_store::BaceWiring;`
  addition.
- `crates/bradian-relational/src/history_backup.rs` — same
  shape.
- `crates/bradian-relational/src/backup.rs` — promote
  `read_receipt` to `pub fn`.
- `crates/bradian-relational/tests/database_backup.rs` —
  new tests.
- `crates/bradian-relational/tests/history_backup_consistency.rs`
  — new tests.

## 8. What this ADR explicitly does NOT do

- Change the coordinator backup-side (`backup_database`,
  `backup_history_database`).
- Change `CatalogLifecycle::open` or any ambient
  coordinator-open entry point.
- Bump manifest format versions.
- Add fields to `ComponentReceipt` /
  `HistoryComponentReceipt`.
- Change sub-archive file names, counts, or lock ordering.
- Introduce archive-level compression, encryption, or
  authentication.
- Attempt to be BACE-aware inside `verify_reopened` (which
  reopens the full coordinator).
- Fix `.derc` or BRWL carriers (separate ADRs).

## 9. Known scope limit explicitly surfaced

End-to-end BACE deployment of a Bradian database requires
`CatalogLifecycle::open_with_bace` so component `RowStore`s
emit BACE sub-archives. **That** is the actual unblocker; this
ADR is the matching restore-side plumbing that will be waiting.

Until the CatalogLifecycle ADR lands:

- Every coordinator backup produced by `backup_database` /
  `backup_history_database` continues to have all-zero
  BackupFlags in every sub-archive (writer emits v1 pages).
- `restore_database` and `restore_history_database` continue
  to work exactly as today.
- `restore_database_with_bace` adds a new entry point that
  tolerates wiring on legacy sub-archives (ADR 0065 §3.5) and
  is ready for BACE sub-archives the day they become
  producible.

## 10. Acceptance evidence required before marking Accepted

1. ADRs 0060–0067 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q1 (naming), Q2 (shared single wiring), Q3
   (`read_receipt` visibility), Q6 (defer
   `verify_reopened` BACE awareness), Q7 (defer top-level
   manifest flag).
3. Implementation satisfying every item in §3.6.
4. No regressions in existing `database_backup.rs`,
   `history_backup_consistency.rs`, or `backup_restore.rs`
   test files.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
