# ADR 0069 — CatalogLifecycle BACE open (implementation; **PROPOSED**)

**Status: PROPOSED — not accepted for implementation.**

Blocked on the accepted BACE prerequisite stack (0060–0063), the
accepted row-store integration (0065), the accepted single-store
backup carrier (0067), and the accepted coordinator restore
carrier (0068). This ADR is the first **writer-side** carrier
after `RowStore::open_with_bace` landed: it threads BACE wiring
through the G18 `CatalogLifecycle` coordinator so a whole
G19 database backup emits BACE sub-archives that the ADR 0068
restore path knows how to consume.

Owner review required before any code lands per CLAUDE.md §4.9.

## 1. Context

- After ADRs 0065, 0067 and 0068 the plumbing exists to **read**
  a BACE-wrapped Bradian database back from a coordinator
  backup. But today nothing produces one: both
  `PersistentCatalog::open_with_limits` and
  `MultiTableStore::open_with_limits` internally call
  `RowStore::open(path)` (the legacy pre-BACE entry point), so
  `CatalogLifecycle::open` composes three legacy stores and
  every coordinator backup it produces has all-zero
  `BackupFlags`.
- This ADR's purpose: ship `CatalogLifecycle::open_with_bace`
  (and the two transitive `_with_bace` opens on its component
  stores) so an operator can run a full Bradian database on BACE,
  back it up, and restore it through ADR 0068's
  `restore_database_with_bace`.
- Only the G19 coordinator (catalog / shared-domain / lifecycle)
  is in scope. G24 (`HistorySqlEngine` + `history_backup.rs`) is
  a separate coordinator and gets its own follow-up ADR.

## 2. Scope of this ADR

**In scope:**

- `PersistentCatalog::open_with_bace` and
  `_with_bace_and_limits` — threads a `BaceWiring` down to the
  inner `RowStore::open_with_bace`.
- `MultiTableStore::open_with_bace` and
  `_with_bace_and_limits` — same.
- `CatalogLifecycle::open_with_bace` — single entry point
  taking one `BaceWiring` that is cloned three times (once per
  component store).
- `CatalogLifecycle::backup_database_to_with_bace(destination)`
  — mirrors the existing `backup_database_to` but uses each
  BACE-wired store's own `backup_current_generation`, which now
  threads wiring through staging-side verification (ADR 0067).
- Receipt flag per component is derived automatically by each
  sub-store's `backup_current_generation` (ADR 0067 §3.3).
- Round-trip: `open_with_bace` → populate → ZSTD profile →
  `backup_database_to_with_bace` → produces a coordinator
  archive whose three sub-archives all have
  `flags.payload_has_bace_pages = true` → restore through ADR
  0068 `restore_database_with_bace` → reopen with
  `open_with_bace` → every row recovered.
- Legacy entry points (`open`, `backup_database_to`) are
  preserved. They still produce flag-clear coordinator backups
  that both restore paths accept.

**Explicitly out of scope** (follow-ups):

- **`HistorySqlEngine::open_with_bace` and
  `backup_to_with_bace`.** G24 coordinator. Separate ADR (0070
  candidate).
- **DomainSqlEngine BACE-opt-in.** `DomainSqlEngine` opens
  through `MultiTableStore::open`, so once
  `MultiTableStore::open_with_bace` ships, a parallel
  `DomainSqlEngine::open_with_bace` is trivially derivable.
  But adding the SQL-engine-facing API is a separate concern
  and keeps this ADR bounded.
- **Policy service identity in the lifecycle marker row.** The
  lifecycle journal stores a format-version marker (`MARKER`
  → `VERSION` row). This ADR does NOT extend the marker to
  capture "this database was last opened with BACE wiring X."
  Operators are responsible for keeping their wiring
  configuration consistent across opens; a mismatched wiring
  will surface as a decode failure, not silent data divergence.
- **Changing the physical layout of catalog / domain /
  lifecycle directories.**
- **Online or concurrent coordinator-wide checkpoints.** The
  existing operation-mutex discipline stays.
- **Authentication, encryption, remote storage.** Out of
  scope for every BACE ADR in this series.
- **Policy per-component wirings.** Draft uses a single
  `BaceWiring` shared across the three components (matches
  ADR 0068 §3.3 shared-wiring model).
- **Making `open` detect an existing BACE directory and refuse
  rather than failing deep inside.** `RowStore::open` already
  fails closed on v2 pages with
  `BaceOpenWithoutBaceWiring { page_id }` (ADR 0065 §3.4). The
  coordinator inherits that failure mode transitively — good
  enough.

## 3. Proposed design

### 3.1 `MultiTableStore::open_with_bace`

```rust
impl MultiTableStore {
    pub fn open_with_bace(
        root: impl AsRef<Path>,
        wiring: BaceWiring,
    ) -> DomainResult<Self> {
        Self::open_with_bace_and_limits(root, wiring, DomainLimits::default())
    }

    pub fn open_with_bace_and_limits(
        root: impl AsRef<Path>,
        wiring: BaceWiring,
        limits: DomainLimits,
    ) -> DomainResult<Self> {
        let storage = RowStore::open_with_bace(
            root, StorageLimits::default(), wiring,
        )?;
        // ... rest of existing open logic unchanged ...
    }
}
```

### 3.2 `PersistentCatalog::open_with_bace`

Symmetric — the catalog is a thin wrapper around a `RowStore`
plus an in-memory snapshot. The only line that changes is
`RowStore::open(path)` → `RowStore::open_with_bace(path, _,
wiring)`.

### 3.3 `CatalogLifecycle::open_with_bace`

```rust
impl CatalogLifecycle {
    pub fn open_with_bace(
        root: impl AsRef<Path>,
        wiring: BaceWiring,
    ) -> LifecycleResult<Self> {
        // Existing pre-flight checks (history-domain refusal,
        // legacy per-table storage refusal) unchanged.
        let catalog = PersistentCatalog::open_with_bace(
            root.join("catalog"), wiring.clone(),
        )?;
        let domain = MultiTableStore::open_with_bace(
            root.join("shared-domain"), wiring.clone(),
        )?;
        let journal = RowStore::open_with_bace(
            root.join("lifecycle"),
            StorageLimits::default(),
            wiring.clone(),
        )?;
        // Marker / reconcile logic unchanged.
    }
}
```

Single shared wiring applies to all three components. The
`BaceWiring` struct is `Clone` already (it holds `Arc`s to the
policy service and registry).

### 3.4 `CatalogLifecycle::backup_database_to_with_bace`

```rust
impl CatalogLifecycle {
    pub fn backup_database_to_with_bace(
        &self,
        destination: impl AsRef<Path>,
    ) -> DatabaseBackupResult<DatabaseBackupReport> {
        // Hold the operation mutex, acquire the three component
        // locks, call database_backup::backup_database.
        // database_backup unchanged — each component's
        // backup_current_generation already picks up the wiring
        // stored on its own RowStore (ADR 0067 §3.3).
    }
}
```

**No change to `database_backup::backup_database`.** The ADR
0067 `RowStore::backup_current_generation` already derives the
flag from `self.bace_wiring` and passes the wiring through to
staging-side verify. When the three component stores were
opened with BACE, each sub-archive automatically carries the
flag.

The legacy `backup_database_to` stays as-is; it uses the
existing component `backup_to` entry points. When the component
stores were opened with legacy `open`, flags stay clear and
legacy restore works. When the components were opened via
`open_with_bace`, the existing `backup_to` still threads wiring
through, so this legacy coordinator entry point ALSO produces
BACE sub-archives. That's the consequence of ADR 0067 §3.3's
"flag derivation on the writer side" — the coordinator did
not need an explicit `_with_bace` variant to produce the right
bytes. The `_with_bace` coordinator entry point is nevertheless
added for explicit clarity and future-proofing.

### 3.5 Error paths

- `RowStore::open_with_bace` fails closed on v1 directories
  that contain v2 pages the wiring can't decode (ADR 0065
  §3.4). This surfaces transitively through
  `MultiTableStore::open_with_bace` →
  `DomainError::Store(StoreError::...)` etc.
- A coordinator previously opened with wiring W and now
  attempted to be opened with legacy `open` will fail inside
  `RowStore::open` with `BaceOpenWithoutBaceWiring`. Every
  existing coordinator-open error surface already carries
  `#[from] StoreError`, so no new error variants are needed.
- A coordinator opened with wiring W1 and attempted to be
  reopened with wiring W2 where W2 doesn't register W1's codecs
  fails with `BaceCodecUnavailable` during the staging-side
  recovery path (same mechanism as ADR 0067).

### 3.6 Compliance obligations (implementation test checklist)

An implementation of this ADR MUST demonstrate every item below:

1. `MultiTableStore::open_with_bace` opens a fresh directory
   and round-trips a simple insert/query.
2. `PersistentCatalog::open_with_bace` opens a fresh directory
   and round-trips a DDL journal entry.
3. `CatalogLifecycle::open_with_bace` opens a fresh directory
   and the `backup_database_identity` triplet reports
   consistent values.
4. A full round-trip: `open_with_bace` → `CREATE TABLE` DDL via
   `SqlEngine` (which opens through DomainSqlEngine → but
   DomainSqlEngine still uses legacy opens — see §4 A) **skip**;
   use the lower-level `provision_existing` + direct RowStore
   mutations instead to keep this ADR independent of the
   SqlEngine stack. Or scope-defer the end-to-end SQL
   round-trip to a sibling ADR.
5. `backup_database_to_with_bace` on a BACE-opt-in
   `CatalogLifecycle` with `CompressionProfile::Balanced`
   produces a coordinator archive where every sub-archive's
   `backup.info` has `flags.payload_has_bace_pages = true`.
6. The same archive restores correctly through ADR 0068
   `restore_database_with_bace`, yielding a destination whose
   reopened `CatalogLifecycle::open_with_bace` returns the
   original rows.
7. `CatalogLifecycle::open` on a destination that was BACE-
   written (test 6) fails closed with
   `LifecycleError::Storage(StoreError::BaceOpenWithoutBaceWiring { .. })`.
8. `backup_database_to` (legacy coordinator entry) on a BACE-
   wired `CatalogLifecycle::open_with_bace` **also produces a
   BACE coordinator backup**, because the component
   `backup_to` calls transitively pick up `self.bace_wiring`.
   Document this behavior with a test that asserts it.
9. A legacy coordinator backup (produced via plain `open` +
   `backup_database_to`) restores through
   `restore_database_with_bace` with wiring attached, exactly
   as ADR 0068 §3.6 item 1 demonstrated.

## 4. Alternatives considered

**A. Open the full `SqlEngine` / `DomainSqlEngine` stack with
BACE in this ADR.** More operator value. Rejected: widens scope
to several more entry points with their own DDL flows. Keep
this ADR to the coordinator-storage boundary.

**B. Make `CatalogLifecycle::open` auto-detect v2 pages and
auto-attach a default wiring.** Simpler UX. Rejected for the
same reason ADR 0066 and 0067 rejected auto-detect: silent
format assumptions violate the "explicit opt-in" culture.

**C. Store the policy-service identity in the lifecycle marker
row so a reopen with a mismatched wiring is detected up front.**
Belt and braces. Rejected for v1: the deep-fail path already
produces a specific, actionable error
(`BaceCodecUnavailable`); adding a marker row tie-in creates
an upgrade/migration obligation.

**D. Add a `backup_database_to_with_wiring(wiring)` form that
takes wiring at backup time, independent of how the
CatalogLifecycle was opened.** Flexibility. Rejected: breaks
the invariant that each sub-store's wiring determines its
on-disk format. Wiring must be chosen at open time.

**E. Per-component wirings.** Flexibility. Rejected per ADR
0068 Q2. Single shared wiring in v1; split if a validated use
case appears.

## 5. Open design questions (owner review)

1. **Entry-point naming.** Draft:
   `open_with_bace` / `backup_database_to_with_bace`. Confirm.
2. **DomainSqlEngine scope.** Draft: out of scope for this
   ADR; a thin follow-up adds `DomainSqlEngine::open_with_bace`
   once this ADR lands. Confirm defer.
3. **Legacy `backup_database_to` on a BACE-wired
   `CatalogLifecycle::open_with_bace`.** Draft: **ALSO
   produces a BACE coordinator backup** (component `backup_to`
   picks up wiring from `self.bace_wiring`). Test and document
   this cleanly instead of trying to hide it behind an
   explicit `_with_bace` coordinator entry point.
   Confirm.
3a. **If Q3 is confirmed, do we need the explicit
    `backup_database_to_with_bace` entry point at all?** Draft:
    YES — keep it for API clarity and to anchor future
    changes (if we add BACE-specific coordinator options,
    they go on the `_with_bace` entry point). Confirm.
4. **Shared single wiring vs. per-component.** Draft: single.
   Confirm (matches ADR 0068 Q2 answered YES).
5. **Marker row carries policy-service identity?** Draft: NO.
   Confirm.
6. **HistorySqlEngine scope.** Draft: out of scope; separate
   follow-up ADR. Confirm.
7. **What happens to `make_database(root)` in tests when a
   BACE-opt-in path is required?** Draft: add a parallel
   `make_bace_database(root, wiring)` in the test file.
   Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A (local).
- **INV-005 recoverable lifecycle:** this ADR is the
  **writer** side that makes ADRs 0067 and 0068 restore-side
  plumbing *reachable*. Every receipt format, flag bit and
  error shape is already in place. No new on-disk format.

## 7. Dependencies this ADR would add

None. No new crate deps.

- `crates/bradian-relational/src/persistent_catalog.rs` — new
  `open_with_bace` entry points.
- `crates/bradian-relational/src/transaction_domain.rs` — new
  `open_with_bace` entry points on `MultiTableStore`.
- `crates/bradian-relational/src/lifecycle.rs` — new
  `CatalogLifecycle::open_with_bace`,
  `backup_database_to_with_bace`.
- `crates/bradian-relational/tests/database_backup.rs` — new
  round-trip tests.

## 8. What this ADR explicitly does NOT do

- Touch any on-disk format.
- Add new policy keys, new codecs, new envelope families.
- Introduce archive-level compression, encryption, or
  authentication.
- Touch `HistorySqlEngine` or `history_backup.rs` writer.
- Touch `DomainSqlEngine` or `SqlEngine` open paths.
- Add new error variants to `LifecycleError`,
  `DomainError`, or `PersistentCatalogError` — all existing
  `#[from] StoreError` plumbing suffices.
- Change the operation-mutex / lock ordering discipline.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0068 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q3 (legacy backup_database_to also produces BACE when
   opened via `_with_bace`) and Q3a (keep `_with_bace`
   coordinator entry point for API clarity even so).
3. Implementation satisfying every item in §3.6.
4. No regressions in `database_backup.rs`,
   `history_backup_consistency.rs`, `backup_restore.rs`, or
   any existing lifecycle / persistent_catalog /
   transaction_domain test file.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
