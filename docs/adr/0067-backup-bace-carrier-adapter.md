# ADR 0067 — Backup carrier BACE adapter (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE prerequisite stack (0060–0063) and
the accepted row-store integration (0065). This ADR is the first
**carrier** that needs an explicit adapter after ADR 0065 landed:
the row-store checkpoint writer now emits BRPG v2 pages carrying
BACE envelopes, and `backup.rs` currently cannot verify such a
backup because its verification path reopens the copied directory
through the legacy `RowStore::open_with_policies` entry point.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- ADR 0065 shipped `RowStore::open_with_bace` and v2 snapshot
  pages. A BACE-opt-in store emits snapshots whose data pages
  carry BACE envelopes with the codec IDs from the operator's
  `allowed_codecs` policy (today RAW and ZSTD; ADR 0066).
- `backup.rs` does a **file-level copy** of
  `checkpoint.current`, `snapshot-{lsn}.brp`, `index-{lsn}.bri`,
  and `rows.wal`. The copy itself already preserves v2 pages
  byte-for-byte.
- But `backup.rs::verify_directory` (called on both the source-
  side backup staging and the destination-side restore staging)
  reopens the copied directory via
  `RowStore::open_with_policies(path, policies)`. That function
  takes the legacy pre-BACE path and fails closed on any v2 page
  with `BaceOpenWithoutBaceWiring` (ADR 0065 §3.4).
- **Net effect today:** backup of a Raw-profile BACE-opt-in store
  works (no v2 pages emitted). Backup of a Balanced / Max /
  non-Raw BACE-opt-in store **fails at verification**, even
  though the copied bytes are correct. Restore has the symmetric
  problem.
- The on-disk backup descriptor (`BRBKv001`, 40 bytes) reserves
  bytes 32–36 with a zeros-are-canonical check at decode time:
  a natural extension point for a `flags` field without changing
  the file's length.

## 2. Scope of this ADR

**In scope:**

- Extend `BackupReceipt` with a 4-byte `flags` field occupying
  the currently-zero bytes 32..36 of the on-disk descriptor. One
  flag bit allocated: `FLAG_PAYLOAD_HAS_BACE_PAGES` (bit 0 of
  byte 32). Remaining 31 bits reserved (must decode as zero).
- Compute the flag at backup time from the source RowStore: set
  iff the RowStore has BACE wiring AND its baseline profile is
  non-Raw. (Cheaper than scanning pages. Writer side knows.)
- Extend `verify_directory` to accept `Option<&BaceWiring>` and
  route it to `RowStore::open_with_bace` when the receipt flag is
  set.
- Extend the public API on both sides:
  - `RowStore::backup_to_with_bace(&self, destination: &Path, policies: StorageLimits) -> StoreResult<BackupReceipt>`
    (writer-side; derives wiring from `self.bace_wiring`).
  - `restore_backup_with_bace(backup, destination, policies, wiring: BaceWiring) -> StoreResult<BackupReceipt>`
    (reader-side).
- Fail-closed semantics:
  - Legacy backup (flag 0) restored via `restore_backup_with_bace`
    with wiring attached — SUCCEEDS. Wiring is tolerated on a
    v1-only directory (matches ADR 0065 §3.5 mixed-generation
    reading).
  - BACE backup (flag 1) restored via the **legacy**
    `restore_backup` without wiring — FAILS CLOSED with a new
    `StoreError::BackupRestoreRequiresBaceWiring` variant.
  - BACE backup (flag 1) restored via `restore_backup_with_bace`
    but the wiring's registry does not include the codec IDs
    referenced by the v2 envelopes — FAILS CLOSED with the
    existing `StoreError::BaceCodecUnavailable` fired during
    verify-time recovery.
  - Reserved flag bits non-zero on decode — FAILS CLOSED with the
    existing `Corruption` variant (matches "zeros canonical"
    rule in the current decoder).
- `BackupReceipt::encode`/`decode` round-trip test covering the
  new flag.

**Explicitly out of scope** (each needs its own ADR or is a
separate carrier):

- **`database_backup.rs`** (whole-database coordinator backup).
  Needs the same treatment but with different plumbing — it
  orchestrates three independent `backup.rs` sub-archives
  (catalog/domain/lifecycle) and currently predates BACE. Follow-
  up ADR (0068 candidate, depending on ordering).
- **`history_backup.rs`** (G22/G23 history-domain backup).
  Separate carrier. Same scope-boundary rule.
- **Compressing the backup archive itself.** This ADR copies v2
  pages that already carry their own BACE envelopes; it does
  NOT re-wrap the `.brp`/`.bri`/`.wal` files in a second BACE
  layer. True archive-level compression (one stream per file or
  one stream per archive) is a separate ambition and deserves
  its own ADR, especially around how the WAL's internal frame
  structure interacts with a streaming codec.
- **Authentication / signing.** ADR 0068 reserved for the
  authenticated envelope family (`BACA` magic). The current
  `BackupReceipt` CRC32 remains the only integrity check.
- **BRBKv002 descriptor version bump.** This ADR uses the
  currently-zero reserved bytes 32..36. The file size and magic
  stay `BRBKv001`. A version bump will happen when we need
  non-additive descriptor changes (e.g., multi-generation
  backups).
- **Per-file encryption.** Separate ADR.
- **Backup-time dictionary training** (BACE §4.2). Separate ADR.
- **Streaming/background backup.** The existing "hold the
  mutation mutex, copy files, verify, publish by rename" model
  stays intact.

## 3. Proposed design

### 3.1 `BackupReceipt` field addition

```rust
pub struct BackupReceipt {
    pub checkpoint_lsn: u64,
    pub last_lsn: u64,
    pub rows: u64,
    pub flags: BackupFlags,         // NEW
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BackupFlags {
    /// Bit 0 — the backup's copied snapshot carries BRPG v2 pages
    /// wrapping BACE envelopes. Restore must supply BACE wiring.
    pub payload_has_bace_pages: bool,
}
```

### 3.2 On-disk descriptor layout (unchanged size, 40 bytes)

| Offset | Size | Field                              |
|-------:|-----:|------------------------------------|
|  0     |  8   | `BRBKv001` magic                   |
|  8     |  8   | `checkpoint_lsn` (u64 LE)          |
| 16     |  8   | `last_lsn` (u64 LE)                |
| 24     |  8   | `rows` (u64 LE)                    |
| **32** |  **4**   | **`flags` (u32 LE)** — new, must be `0` or exactly one of the known bits set |
| 36     |  4   | CRC32 of bytes 0..36               |

Decode rule: any non-zero bit outside the known mask is a
`Corruption("reserved backup flag bits non-zero")` error —
preserves the existing fail-closed culture on unknown descriptor
state.

### 3.3 Writer side

```rust
impl RowStore {
    pub fn backup_to_with_bace(
        &self,
        destination: &Path,
        policies: StorageLimits,
    ) -> StoreResult<BackupReceipt> { ... }
}
```

Implementation sketch:

1. Take the mutation mutex, same as `backup_to`.
2. Build `BackupReceipt { .., flags: BackupFlags { payload_has_bace_pages: self.has_bace_v2_writer() } }`.
3. Call `backup::create_backup` passing `self.bace_wiring.as_ref()`
   as an extra parameter to be used during `verify_directory`.

`has_bace_v2_writer()` returns `true` iff
`self.bace_wiring.is_some()` **and** the resolved profile from
`resolve_bace_codec` returned `Some((_, _))` at the latest
checkpoint. A Raw-profile BACE-opt-in store has wiring but emits
v1 pages — flag stays clear. Simpler alternative: flag is just
`self.bace_wiring.is_some() && profile != Raw`.

The legacy `RowStore::backup_to(destination, policies)` still
exists and uses the legacy `backup::create_backup` path (no
wiring, flag cleared). If a BACE-opt-in store with non-Raw
profile calls the legacy path, the staging-side verification
fails exactly as it does today — surfacing the mismatch instead
of silently producing an un-verifiable backup. A follow-up could
make the legacy entry point fail earlier with a specific error;
left to the implementation discretion.

### 3.4 Reader side

```rust
pub fn restore_backup_with_bace(
    backup: &Path,
    destination: &Path,
    policies: StorageLimits,
    wiring: BaceWiring,
) -> StoreResult<BackupReceipt> { ... }
```

Legacy `restore_backup` / `restore_backup_with_policies` are kept
but:

- If the receipt's `flags.payload_has_bace_pages` is set, they
  return `StoreError::BackupRestoreRequiresBaceWiring` **before
  touching the destination**. No staging directory is created.

`restore_backup_with_bace`:

- If `flags.payload_has_bace_pages` is set — pass the wiring to
  `verify_directory`.
- If the flag is clear — pass the wiring anyway. ADR 0065 §3.5
  mixed-generation guarantees that `open_with_bace` on a v1-only
  directory works. This keeps the restore API uniform for
  operators.

### 3.5 `verify_directory` signature

```rust
fn verify_directory(
    path: &Path,
    receipt: BackupReceipt,
    expected_rows: Option<&BTreeMap<Vec<u8>, Vec<u8>>>,
    policies: StorageLimits,
    bace: Option<&BaceWiring>,         // NEW
) -> StoreResult<()>
```

Routing rule inside:

```rust
let recovered = match bace {
    Some(w) => RowStore::open_with_bace(path, policies, w.clone())?,
    None    => RowStore::open_with_policies(path, policies)?,
};
```

The rest of the function (WAL-tail-repair rejection, LSN/row
check, `verify_checkpoint_index`, `visit_snapshot` comparison)
stays unchanged.

### 3.6 New error variant

```rust
// src/row_store.rs StoreError enum
#[error("backup carries BACE v2 pages; restore_backup_with_bace is required")]
BackupRestoreRequiresBaceWiring,
```

Fail-closed, specific, actionable. Operators see an error that
tells them exactly which entry point to use.

### 3.7 Compliance obligations (implementation test checklist)

An implementation of this ADR MUST demonstrate every item below:

1. `BackupReceipt::encode`/`decode` round-trip with the new flag
   cleared yields a legacy-shape 40-byte descriptor (all four
   flag bytes zero).
2. Round-trip with `payload_has_bace_pages = true` yields exactly
   `0x01, 0x00, 0x00, 0x00` at bytes 32..36.
3. Decode rejects any reserved bit non-zero (e.g., bytes 32..36 =
   `0x02, 0x00, 0x00, 0x00`) with `Corruption(..)`.
4. Decode rejects a legacy-era backup descriptor whose CRC was
   computed over `flags = 0` but whose flag bytes are now
   non-zero on disk (CRC mismatch).
5. `backup_to_with_bace` on a Raw-profile BACE-opt-in store
   produces a descriptor with the flag clear; the backup is
   restorable through the legacy `restore_backup` path.
6. `backup_to_with_bace` on a Balanced-profile BACE-opt-in store
   produces a descriptor with the flag set; the backup is NOT
   restorable through the legacy `restore_backup` path (fails
   closed with `BackupRestoreRequiresBaceWiring`, no staging
   directory created at the destination).
7. The same Balanced-profile backup IS restorable through
   `restore_backup_with_bace` with wiring that includes the
   source's codec set; restored store answers every key with the
   original value.
8. Restoring a BACE backup with wiring whose registry lacks the
   codec IDs referenced by the v2 envelopes fails closed with
   `BaceCodecUnavailable` during verification; no destination is
   published.
9. Restoring a **legacy (pre-BACE) v1-only** backup through
   `restore_backup_with_bace` SUCCEEDS — wiring is tolerated on
   a v1 directory per ADR 0065 §3.5.
10. Source-side verification still rejects a BACE-opt-in backup
    created via the **legacy** `backup_to` path (no wiring
    plumbed through verify) — the existing behavior is
    preserved; a follow-up may add a specific error.

## 4. Alternatives considered

**A. Bump descriptor magic to `BRBKv002`.** Clean separation,
room to grow. Rejected: the current reserved bytes 32..36 solve
the immediate need without touching readers of
legacy-but-still-valid backups. A version bump is reserved for
non-additive changes.

**B. Compress the whole backup archive with ZSTD.** Big space
savings on the WAL and non-page files. Rejected for this ADR:
different ambition, different threat model, and the question of
"compressing a file that already contains per-page BACE
envelopes" deserves its own design.

**C. Scan the snapshot file to set the BACE flag.** Robust to
future opt-in paths that don't carry explicit wiring. Rejected:
unnecessary IO at backup time. The writer already knows the
wiring state; the extra scan has no upside.

**D. Make `restore_backup` auto-attach a default BACE wiring if
the flag is set.** Simpler operator API. Rejected: "silent
format change" is exactly the thing the ADR 0066 default
`allowed_codecs = [RAW_ID]` policy was built to prevent.
Operators must opt in explicitly.

**E. Change `backup.rs` to always emit a BACE-wrapped archive
regardless of the source.** Uniform shape. Rejected: breaks
byte-level restore compatibility with every previously-created
backup. The flag approach is purely additive.

## 5. Open design questions (owner review)

1. **New API naming.** Draft: `RowStore::backup_to_with_bace` and
   `restore_backup_with_bace`. Alternatives:
   `backup_to_including_bace`, `backup_bace`, overloading
   `backup_to` (not possible in Rust without a builder).
   Confirm naming.
2. **Flag derivation.** Draft: set iff wiring is present AND
   profile is non-Raw AT BACKUP TIME. Alternative: scan the
   snapshot file for any v2 page. Confirm the simpler rule.
3. **Legacy `backup_to` on a non-Raw BACE store.** Draft:
   preserve current behavior (verification fails inside
   `RowStore::open_with_policies` on the staging copy, so the
   backup is never published). Alternative: add an explicit
   upfront error. Confirm the conservative no-new-error stance,
   or request the explicit error.
4. **Reserved-bit strictness.** Draft: any non-zero reserved bit
   fails decode. Alternative: ignore unknown bits (forward-
   compat). Confirm strict behavior (matches the existing
   "zeros canonical" rule).
5. **Include BACE wiring's policy service identity in the
   receipt?** Draft: NO. Only the fact that BACE was used.
   Alternative: hash the baseline policy and record it so restore
   can warn on policy drift. Confirm NO (keeps the receipt
   stable).
6. **`database_backup.rs` adapter scope.** Draft: out of scope,
   separate follow-up. Alternative: do it in the same ADR.
   `database_backup.rs` is a 348-line coordinator over three
   separate `backup.rs` invocations; deferring keeps this ADR
   bounded. Confirm the defer.
7. **Test fixture compatibility.** Legacy backups created before
   this ADR will decode to `flags = 0` (the reserved bytes were
   zero and the CRC covered them). Confirm no corpus migration
   is required.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change. Backup still
  captures an LSN-consistent pair (checkpoint, WAL tail).
- **INV-002 one authoritative state:** no change. Restore still
  publishes only after verification.
- **INV-003 no authorization gap:** no change. BACE envelopes
  carry no authorization context; the receipt CRC and
  rename-after-verify discipline remain the guards.
- **INV-004 no unsafe automatic availability:** N/A (local).
- **INV-005 recoverable lifecycle:** the new flag is additive;
  legacy backups (flag 0) remain restorable through every API.
  BACE backups require the explicit `_with_bace` entry point —
  the same rule ADR 0065 applied to `RowStore::open_with_bace`.

## 7. Dependencies this ADR would add

None. No new crate deps. The implementation is confined to:

- `crates/bradian-relational/src/backup.rs`
- `crates/bradian-relational/src/row_store.rs` (one new
  `backup_to_with_bace` method + one new `StoreError` variant)
- `crates/bradian-relational/tests/backup_restore.rs` (new
  tests)

## 8. What this ADR explicitly does NOT do

- Touch `database_backup.rs` or `history_backup.rs`.
- Change the on-disk file-copy set (still
  `checkpoint.current` + `snapshot-{lsn}.brp` +
  `index-{lsn}.bri` + `rows.wal` + `backup.info`).
- Introduce archive-level compression or encryption.
- Change the `BRBKv001` descriptor magic.
- Add new policy keys.
- Alter the "hold the mutation mutex, copy, verify, rename"
  discipline.
- Change how `verify_checkpoint_index` works (it already runs
  through the reopened `RowStore`, so it inherits BACE decode
  transparently).

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0066 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially Q1
   (naming), Q2 (flag derivation), Q3 (legacy-path behavior),
   and Q6 (defer `database_backup.rs`).
3. Implementation satisfying every item in §3.7.
4. No regressions in existing `backup_restore.rs`,
   `database_backup.rs`, and `history_backup_consistency.rs`
   test files.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
