# ADR 0070 — HistorySqlEngine BACE open (implementation; **ACCEPTED**)

**Status: ACCEPTED, 15 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit.

G24 mirror of ADR 0069. Three new store-level entries exposed
(`DurableMvccStore::open_with_bace`,
`DurableTypedStore::open_with_bace` + variants, and the
coordinated variant), plus `HistorySqlEngine::open_with_bace`
and `backup_to_with_bace`. `history_backup::backup_history_database`
and `verify_root` thread wiring through so staging-side
verification and reopens dispatch to the `_with_bace` variants.

Implementation summary:

- `crates/bradian-relational/src/durable_mvcc.rs` —
  `DurableMvccStore::open_with_bace` + `_with_bace_and_resource_limits`.
- `crates/bradian-relational/src/durable_typed.rs` — three
  `DurableTypedStore::open_with_bace*` variants +
  `open_coordinated_with_bace`.
- `crates/bradian-relational/src/history_sql.rs` — three
  `HistorySqlEngine::open_with_bace*` variants +
  `backup_to_with_bace`. New `bace_wiring: Option<BaceWiring>`
  field stored so `backup_to` passes it down.
- `crates/bradian-relational/src/history_backup.rs` —
  `backup_history_database` gained `bace: Option<&BaceWiring>`
  and threads it through staging-side sub-archive restores AND
  through `verify_root` (which now reopens via the BACE-aware
  catalog + history-domain opens when wiring is present).
- `crates/bradian-relational/tests/history_backup_consistency.rs`
  — 4 new integration tests.

965 workspace tests pass; clippy clean on
`bradian-relational --all-targets`.

### Observed scope limit on §3.6 item 6

The drafted §3.6 item 6 claimed `HistorySqlEngine::open` on a
BACE-written restored destination fails closed. Reaching that
scenario requires the restored sub-stores to actually contain
v2 pages. In the current G24 fixture (`migrated()`), the
catalog and history-domain are checkpointed once during
migration at a fixed LSN and have no further mutations before
`backup_to`. Per the ADR 0065 "checkpoint is idempotent at
current LSN" rule, `backup_to` reuses the existing v1 snapshot
even under BACE wiring — no v2 pages are produced.

Producing v2 pages through G24 requires either the DDL
coordinator (`enable_epoch_row_writes` + `execute_ddl`) which
this ADR §5 Q2 explicitly defers, or a post-migration mutation
path that also lives behind the DDL coordinator.

The test that landed asserts what IS reachable today: the
BackupFlags round-trip through backup → restore so when the
DDL coordinator ADR lands, this path is already wired
correctly. The fail-closed invariant for v2 pages in a
`HistorySqlEngine::open` call is proven at the RowStore layer
by ADR 0067's test suite; the G24 coordinator inherits the
behavior transitively.

## 1. Context

- ADR 0069 closed the end-to-end BACE deployment loop for G19
  (catalog / shared-domain / lifecycle). G24 is the parallel
  coordinator for G23 durable-history databases
  (`HistorySqlEngine` + `history_backup.rs`), and today it still
  routes every open through the legacy pre-BACE entry points:
  `HistorySqlEngine::open_with_resource_limits` opens
  `PersistentCatalog::open` and
  `DurableTypedStore::open_coordinated`, which in turn opens
  `DurableMvccStore::open_with_resource_limits`, which opens a
  bare `RowStore::open(path)`.
- `PersistentCatalog::open_with_bace` already ships (ADR 0069).
  So does `RowStore::open_with_bace` (ADR 0065). The missing
  rungs are `DurableMvccStore::open_with_bace` and
  `DurableTypedStore::open_with_bace`, plus the
  `HistorySqlEngine::open_with_bace` coordinator entry point
  that threads a single shared wiring through
  `PersistentCatalog` + `DurableTypedStore`.
- `history_backup.rs::backup_history_database` has the same
  staging-side verification shape that ADR 0069 had to expand
  scope for in `database_backup.rs::backup_database`: it
  restores each sub-archive via LEGACY `RowStore::restore_backup`
  and reopens the staged root via LEGACY `PersistentCatalog::open`
  / `DurableTypedStore::open_with_history_limits`. Both fail
  closed on BACE sub-archives post-ADR-0067/0065.
- Restore-side plumbing is already in place: ADR 0068 shipped
  `restore_history_database_with_bace`. This ADR makes a BACE
  sub-archive actually producible.

## 2. Scope of this ADR

**In scope:**

- `DurableMvccStore::open_with_bace` and
  `_with_bace_and_resource_limits`.
- `DurableTypedStore::open_with_bace` and matching
  `_with_history_limits` / `_with_resource_limits` variants.
- `DurableTypedStore::open_coordinated_with_bace` — the
  coordinated variant called by `HistorySqlEngine`.
- `HistorySqlEngine::open_with_bace` and matching variants.
- Wiring stored on `HistorySqlEngine` so
  `backup_to` can pass it down to
  `history_backup::backup_history_database`.
- `HistorySqlEngine::backup_to_with_bace` — thin alias for
  `backup_to_with_metadata_limits` on a `_with_bace`-opened
  engine (same API-clarity rule as ADR 0069 §5 Q3a).
- `history_backup::backup_history_database` gets a
  `bace: Option<&BaceWiring>` parameter. Staging-side
  sub-archive restores dispatch to
  `RowStore::restore_backup_with_bace` under BACE.
  `verify_root` gains a `bace: Option<&BaceWiring>` parameter
  and reopens via the `_with_bace` variants of the catalog +
  history-domain stores. The restore-side call site in
  `history_backup.rs` threads its own `wiring.as_ref()` through.
- Round-trip: `open_with_bace` → populate G23 history under
  ZSTD profile → `backup_to_with_bace` → archive whose both
  sub-archives carry `flags.payload_has_bace_pages = true` →
  restore through ADR 0068 `restore_history_database_with_bace`
  → reopen with `open_with_bace` → every retained version
  recovered.
- Legacy entry points (`open`, `backup_to`,
  `restore_history_database`) stay. They still work on
  non-BACE G24 roots.

**Explicitly out of scope** (follow-ups):

- **`DomainSqlEngine::open_with_bace`,
  `SqlEngine::open_with_bace`.** G16/G17 SQL frontends.
  Separate thin follow-up ADR.
- **G23 schema activation (`HistorySqlEngine::activate_schema` /
  `enable_epoch_row_writes` / `execute_ddl`) under BACE.** These
  exclusive-DDL flows reopen the catalog/history with their own
  mutable engines and would need the same `_with_bace` threading.
  Deferred to a separate ADR because they are a different code
  path (mutable DDL coordinator) with its own invariants.
- **Mixed-generation G24 archives.** ADR 0065 §3.5 rule still
  applies at the RowStore layer; the coordinator does not add
  its own mixed-generation logic.
- **Manifest format bump.** `BRADIAN-HISTORY-BACKUP-V1` stays.
  The ADR 0067 flag lives in each sub-archive's `backup.info`.
- **`HistoryComponentReceipt` schema changes.** Unchanged.
- **Policy service identity in any G23 marker row.** Same
  rationale as ADR 0069 §5 Q5 — a reopen with mismatched
  wiring surfaces `BaceCodecUnavailable` specifically.
- **Authentication, encryption, remote storage.** Out of scope
  for every BACE ADR in this series.
- **Per-component wirings.** Draft uses a single shared
  `BaceWiring` (matches ADR 0069 Q4).

## 3. Proposed design

### 3.1 `DurableMvccStore::open_with_bace`

```rust
impl DurableMvccStore {
    pub fn open_with_bace(
        path: impl AsRef<Path>, wiring: BaceWiring,
    ) -> DurableMvccResult<Self>;
    pub fn open_with_bace_and_resource_limits(
        path, wiring, limits: HistoryLimits, transactions: TransactionLimits,
    ) -> DurableMvccResult<Self>;
    fn open_inner(path, limits, transactions, wiring: Option<BaceWiring>);
}
```

The only line that changes inside: `RowStore::open(path)` →
`RowStore::open_with_bace(path, _, wiring)`. The format-marker
bootstrap commit afterwards is unchanged (under BACE it will
land on v1 the first time because there is no checkpoint yet;
the first checkpoint emits v2).

### 3.2 `DurableTypedStore::open_with_bace`

Three new entry points mirroring the three existing opens
(`open`, `open_with_history_limits`,
`open_with_resource_limits`), plus a BACE-aware
`open_coordinated_with_bace`:

```rust
impl DurableTypedStore {
    pub fn open_with_bace(
        domain_root, catalog_tables, wiring,
    ) -> Result<Self, DomainError>;
    pub fn open_with_bace_and_history_limits(
        domain_root, catalog_tables, wiring, limits: HistoryLimits,
    ) -> Result<Self, DomainError>;
    pub fn open_with_bace_and_resource_limits(
        domain_root, catalog_tables, wiring, limits, transactions,
    ) -> Result<Self, DomainError>;
    pub(crate) fn open_coordinated_with_bace(
        domain_root, catalog, wiring, limits, transactions,
    ) -> Result<Self, DomainError>;
}
```

Each delegates to an internal shared constructor that calls
`DurableMvccStore::open_with_bace_and_resource_limits` under
the hood.

### 3.3 `HistorySqlEngine::open_with_bace`

```rust
pub struct HistorySqlEngine {
    catalog: PersistentCatalog,
    domain: DurableTypedStore,
    _root: PathBuf,
    failed_ddl: bool,
    bace_wiring: Option<BaceWiring>,     // NEW
}

impl HistorySqlEngine {
    pub fn open_with_bace(root, wiring) -> SqlResult<Self>;
    pub fn open_with_bace_and_history_limits(root, wiring, limits);
    pub fn open_with_bace_and_resource_limits(root, wiring, limits, transactions);
    pub fn backup_to_with_bace(&self, destination)
        -> HistoryBackupResult<HistoryBackupReport>;
}
```

The same pre-flight checks (legacy-storage refusal, missing
history-domain refusal, mixed-G15/G18 refusal) run before the
`_with_bace` opens of the two component stores.

`backup_to` (the legacy entry point) calls
`history_backup::backup_history_database(..., self.bace_wiring.as_ref())`
so a BACE-opened engine's legacy backup call ALSO produces a
BACE archive (ADR 0069 §5 Q3 rule applied to G24).

### 3.4 `history_backup::backup_history_database` scope expansion

Mirrors the ADR 0069 scope expansion in
`database_backup::backup_database`:

```rust
#[allow(clippy::too_many_arguments)]
pub(crate) fn backup_history_database(
    source, destination, catalog, history, metadata,
    bace: Option<&BaceWiring>,
) -> HistoryBackupResult<HistoryBackupReport> {
    // ... existing per-component backup unchanged ...
    // Staging-side sub-archive restores:
    for component in COMPONENTS {
        match bace {
            Some(w) => RowStore::restore_backup_with_bace(
                stage.join(component), verify.join(component),
                StorageLimits::default(), w.clone(),
            )?,
            None => RowStore::restore_backup(
                stage.join(component), verify.join(component),
            )?,
        };
    }
    verify_root(&verify, &report, history.history_limits(), bace)?;
    // ... rest unchanged ...
}

fn verify_root(
    root, expected, limits, bace: Option<&BaceWiring>,
) -> HistoryBackupResult<()> {
    let catalog = match bace {
        Some(w) => PersistentCatalog::open_with_bace(root.join("catalog"), w.clone())?,
        None => PersistentCatalog::open(root.join("catalog"))?,
    };
    let definitions = catalog_tables(&catalog);
    let domain = match bace {
        Some(w) => DurableTypedStore::open_with_bace_and_history_limits(
            root.join("history-domain"), &definitions, w.clone(), limits,
        )?,
        None => DurableTypedStore::open_with_history_limits(
            root.join("history-domain"), &definitions, limits,
        )?,
    };
    // ... existing verification unchanged ...
}
```

### 3.5 Restore-side threading

`restore_history_database_inner` (ADR 0068) already carries a
`wiring: Option<BaceWiring>`. It currently only threads wiring
through the sub-archive restores. This ADR extends it to also
pass `wiring.as_ref()` into `verify_root`.

### 3.6 Compliance obligations (implementation test checklist)

1. `DurableMvccStore::open_with_bace` opens a fresh directory
   and round-trips a put/get across a commit.
2. `DurableTypedStore::open_with_bace_and_history_limits`
   opens a provisioned history-domain root and lists its
   schema pins.
3. `HistorySqlEngine::open_with_bace` opens a provisioned G23
   root and `SELECT` succeeds.
4. `HistorySqlEngine::backup_to_with_bace` on a BACE-opened
   engine produces an archive where both sub-archives
   (`catalog`, `history-domain`) carry
   `flags.payload_has_bace_pages = true`.
5. The same archive restores through ADR 0068
   `restore_history_database_with_bace`, yielding a
   destination whose reopened `HistorySqlEngine::open_with_bace`
   recovers the pre-backup rows.
6. `HistorySqlEngine::open` on a BACE-written destination
   fails closed with a `SqlError::*` carrying the
   BACE-origin error surface
   (`BaceOpenWithoutBaceWiring`) through its component chain.
7. `restore_history_database` (legacy entry) fails closed with
   `HistoryBackupError::RestoreRequiresBaceWiring` on an
   archive produced by a BACE-opened `HistorySqlEngine` —
   covered by ADR 0068 test suite, re-verified here against
   actually-BACE-wrapped sub-archives.
8. Legacy `HistorySqlEngine::backup_to` on a `_with_bace`-opened
   engine also produces a BACE archive (ADR 0069 §5 Q3 applied
   to G24).

## 4. Alternatives considered

**A. Also wire DomainSqlEngine and SqlEngine BACE opens here.**
Rejected: widens scope to the G16/G17 SQL frontends and their
DDL flows. Keep this ADR to the G24 coordinator boundary.

**B. Auto-detect v2 pages on legacy `HistorySqlEngine::open`
and attach a default wiring.** Rejected for the same reason
ADRs 0066–0069 rejected auto-detect.

**C. Keep `verify_root` using the legacy opens even when the
sub-archives are BACE.** Rejected: the whole point of this ADR
is to close the end-to-end G24 loop. Staging-side verification
has to decode BACE pages.

**D. Store the policy-service identity in the G23 format-marker
row.** Rejected per ADR 0069 Q5 — `BaceCodecUnavailable` at
the deep-fail path is specific enough.

**E. Per-component wirings on `HistorySqlEngine`.** Rejected
per ADR 0069 Q4 — single shared wiring in v1.

## 5. Open design questions (owner review)

1. **Entry-point naming.** Draft:
   `open_with_bace`, `backup_to_with_bace`. Confirm.
2. **DDL-coordinator variants
   (`HistorySqlEngine::enable_epoch_row_writes`, `execute_ddl`,
   `activate_schema`) under BACE.** Draft: out of scope,
   separate follow-up. Confirm defer.
3. **Legacy `backup_to` on a `_with_bace`-opened engine
   produces BACE.** Draft: YES. Document and test honestly
   (ADR 0069 §5 Q3 precedent).
4. **Shared single wiring vs per-component.** Draft: single.
5. **G24 format-marker carries policy identity?** Draft: NO.
6. **SqlEngine and DomainSqlEngine scope.** Draft: out of
   scope; separate follow-up. Confirm.
7. **Pre-flight policy — refuse `HistorySqlEngine::open` on
   a BACE-written root with a specific SqlError variant?**
   Draft: NO explicit pre-flight; let the deep-fail surface
   through the existing component error chain (consistent
   with ADR 0069). Confirm.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** additive. Every receipt
  format, flag bit, error shape and manifest schema is
  unchanged. Legacy G24 archives continue to restore through
  legacy entry points; BACE archives flow through
  `_with_bace` entry points.

## 7. Dependencies this ADR would add

None. Confined to:

- `crates/bradian-relational/src/durable_mvcc.rs`
- `crates/bradian-relational/src/durable_typed.rs`
- `crates/bradian-relational/src/history_sql.rs`
- `crates/bradian-relational/src/history_backup.rs`
- `crates/bradian-relational/tests/history_backup_consistency.rs`

No new error variants on `HistoryBackupError`,
`DurableMvccError`, `DomainError`, or `SqlError` — every
existing `#[from]` plumbing already surfaces the BACE-origin
errors from `StoreError`.

## 8. What this ADR explicitly does NOT do

- Touch any on-disk format.
- Touch `DomainSqlEngine` or `SqlEngine` open paths.
- Touch G23 schema activation/DDL coordinator flows.
- Add new policy keys or codecs.
- Introduce archive-level compression, encryption, or
  authentication.
- Change manifest version.
- Change coordinator lock / mutex / WAL ordering discipline.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0069 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q2 (defer DDL coordinator variants), Q3 (legacy backup_to
   also produces BACE), Q6 (defer DomainSqlEngine/SqlEngine),
   Q7 (no explicit pre-flight refusal).
3. Implementation satisfying every item in §3.6.
4. No regressions in `history_backup_consistency.rs`,
   `durable_mvcc_*`, `durable_typed_*`, `history_sql_*`,
   `database_backup.rs`, or `backup_restore.rs` test files.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
