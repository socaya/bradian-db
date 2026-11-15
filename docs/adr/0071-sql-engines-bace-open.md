# ADR 0071 — SQL engines BACE open (implementation; **ACCEPTED**)

**Status: ACCEPTED, 15 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit.

Operator-facing SQL engines now reach the BACE plumbing landed
under ADRs 0065–0070:

- `TableStore::open_with_bace` — thin wrapper around
  `RowStore::open_with_bace`, preserves schema-pin reconciliation.
- `DomainSqlEngine::open_with_bace` + `_and_limits` — threads
  shared wiring to `PersistentCatalog::open_with_bace` and
  `MultiTableStore::open_with_bace`. No wiring stored on engine.
- `SqlEngine::open_with_bace` — stores
  `bace_wiring: Option<BaceWiring>` on the engine; lazy
  `table_store()` threads it through to
  `TableStore::open_with_bace`.

No new error variants, no on-disk format change, no backup
paths added. Operators who want coordinator backup still go
through `CatalogLifecycle::backup_database_to` (ADR 0069).

Implementation summary:

- `crates/bradian-relational/src/typed_table.rs`
- `crates/bradian-relational/src/domain_sql.rs`
- `crates/bradian-relational/src/sql.rs`
- `crates/bradian-relational/tests/typed_table.rs` — 1 new test
- `crates/bradian-relational/tests/domain_sql.rs` — 2 new tests
- `crates/bradian-relational/tests/sql_execution.rs` — 2 new tests

970 workspace tests pass; clippy clean on
`bradian-relational --all-targets`.

## 1. Context

- After ADR 0069, `CatalogLifecycle::open_with_bace` and the
  three underlying component `_with_bace` opens exist. An
  operator opening the G19 coordinator sees end-to-end BACE.
- After ADR 0070, `HistorySqlEngine::open_with_bace` closes
  the G24 coordinator loop.
- But most day-to-day Bradian operators don't open
  `CatalogLifecycle` directly. They open `SqlEngine` (G14 per-
  table legacy path) or `DomainSqlEngine` (G16 shared-domain
  path) — and those two still route through legacy
  `PersistentCatalog::open` /
  `MultiTableStore::open_with_limits` /
  `TableStore::open`, which means a BACE-opt-in Bradian store
  is unreachable from the SQL frontends.
- This ADR is a thin derivative: expose
  `SqlEngine::open_with_bace` and
  `DomainSqlEngine::open_with_bace` that thread a single
  shared `BaceWiring` through the same component opens the
  coordinators already use.
- `SqlEngine` (G14 per-table) lazily opens one `TableStore`
  per queried table through `SqlEngine::table_store()`.
  Threading wiring there requires storing it on the engine so
  each lazy `TableStore::open` sees it. `TableStore` therefore
  needs `TableStore::open_with_bace` too.

## 2. Scope of this ADR

**In scope:**

- `TableStore::open_with_bace(path, table, wiring)` — thin
  wrapper around `RowStore::open_with_bace`.
- `SqlEngine::open_with_bace(root, wiring)` + a limit variant.
  Stores `bace_wiring: Option<BaceWiring>` on the engine so
  the lazy `table_store()` call threads it through.
- `DomainSqlEngine::open_with_bace(root, wiring)` + a limit
  variant. Opens `PersistentCatalog::open_with_bace` +
  `MultiTableStore::open_with_bace` using the shared wiring.
- `DomainSqlEngine` and `SqlEngine` do NOT get their own
  BACE-aware backup path; operators who want a coordinator
  backup already go through `CatalogLifecycle::backup_database_to`
  (ADR 0069) or through per-table `backup_to` on `TableStore`
  (which inherits ADR 0067 behavior through
  `RowStore::backup_current_generation`).
- Tests covering §3.6 checklist.

**Explicitly out of scope** (each needs its own ADR):

- **`MvccSqlEngine::open_with_bace`.** G21 in-memory MVCC
  engine wraps `TypedMvccStore` → `MvccStore` → `RowStore`.
  Requires `MvccStore::open_with_bace` and
  `TypedMvccStore::open_with_bace` as prerequisites. Follow-up
  ADR.
- **G23 DDL coordinator BACE**
  (`HistorySqlEngine::enable_epoch_row_writes`,
  `execute_ddl`, `activate_schema`). These exclusive-DDL flows
  reopen the catalog/history-domain with their own mutable
  engines. Separate ADR — it is also what would unblock the
  observed scope limit from ADR 0070 §6 item 6.
- **Introducing a BACE-aware `backup_to` on `SqlEngine` or
  `DomainSqlEngine`.** Current operator flows go through
  `CatalogLifecycle`.
- **Mandatory marker row carrying policy identity.** Same
  rationale as ADR 0069/0070: a reopen with mismatched wiring
  surfaces `BaceCodecUnavailable` specifically.
- **Changing the per-table directory layout under
  `root/tables/<id>/`.**
- **Authentication, encryption, remote storage.** Out of
  scope for every BACE ADR in this series.
- **Per-component wirings.** Draft uses a single shared
  `BaceWiring` (matches ADRs 0068/0069/0070).

## 3. Proposed design

### 3.1 `TableStore::open_with_bace`

```rust
impl TableStore {
    pub fn open_with_bace(
        path: impl AsRef<Path>,
        table: Table,
        wiring: BaceWiring,
    ) -> TableResult<Self> {
        let pk_positions = validate_table(&table)?;
        let storage = RowStore::open_with_bace(
            path, StorageLimits::default(), wiring,
        )?;
        // ... existing META_KEY schema-pin reconciliation
        // unchanged ...
    }
}
```

No behavioral change to the schema-pin dance that follows the
storage open.

### 3.2 `DomainSqlEngine::open_with_bace`

```rust
impl DomainSqlEngine {
    pub fn open_with_bace(root, wiring) -> SqlResult<Self> {
        Self::open_with_bace_and_limits(
            root, wiring, DomainLimits::default(), CatalogLimits::default(),
        )
    }

    pub fn open_with_bace_and_limits(
        root, wiring, domain_limits, catalog_limits,
    ) -> SqlResult<Self> {
        // ... existing pre-flight checks unchanged ...
        let catalog = PersistentCatalog::open_with_bace_and_limits(
            root.join("catalog"), wiring.clone(), catalog_limits,
        )?;
        let domain = MultiTableStore::open_with_bace_and_limits(
            root.join("shared-domain"), wiring, domain_limits,
        )?;
        Ok(Self { catalog, domain, operation: Mutex::new(()), _root: root.to_owned() })
    }
}
```

No wiring stored on the engine — both component opens are
eager (not lazy) and their `BaceWiring` is captured inside the
component `RowStore`s they own. The engine doesn't open any
additional stores at session time.

### 3.3 `SqlEngine::open_with_bace`

```rust
pub struct SqlEngine {
    root: PathBuf,
    catalog: PersistentCatalog,
    operation: Mutex<()>,
    bace_wiring: Option<BaceWiring>,     // NEW
}

impl SqlEngine {
    pub fn open_with_bace(root, wiring) -> SqlResult<Self> {
        // ... existing pre-flight checks unchanged ...
        let catalog = PersistentCatalog::open_with_bace(
            root.as_ref().join("catalog"), wiring.clone(),
        )?;
        Ok(Self {
            root: root.as_ref().to_owned(),
            catalog, operation: Mutex::new(()),
            bace_wiring: Some(wiring),
        })
    }

    fn table_store(&self, table: &Table) -> SqlResult<TableStore> {
        let path = self.root.join("tables").join(table.id.0.to_string());
        Ok(match &self.bace_wiring {
            Some(w) => TableStore::open_with_bace(path, table.clone(), w.clone())?,
            None => TableStore::open(path, table.clone())?,
        })
    }
}
```

The lazy table-store open path is the only mutation point;
each `table_store()` call threads the wiring through.

### 3.4 Legacy compatibility invariants

- `DomainSqlEngine::open` and `SqlEngine::open` are unchanged.
  A directory written through the legacy path continues to
  reopen through the legacy path.
- `DomainSqlEngine::open_with_bace` and
  `SqlEngine::open_with_bace` tolerate v1-only directories per
  ADR 0065 §3.5 (inherited from each component store's
  `open_with_bace`).
- `DomainSqlEngine::open` on a directory whose components have
  written v2 pages fails closed through the existing
  component error chain (`PersistentCatalogError::Storage` →
  `DomainError::Storage` → `SqlError`), surfacing
  `BaceOpenWithoutBaceWiring` with a page-id.
- Same for `SqlEngine::open` on a directory whose per-table
  stores have v2 pages. The failure fires lazily inside the
  first `table_store()` call that targets a BACE-written
  table.

### 3.5 Compliance obligations (implementation test checklist)

An implementation of this ADR MUST demonstrate every item:

1. `TableStore::open_with_bace` on a fresh path with a simple
   two-column schema round-trips a put/get across a commit.
2. `DomainSqlEngine::open_with_bace` on a fresh path runs the
   full catalog DDL + domain DML flow
   (`CREATE DATABASE/SCHEMA/TABLE`, `register_catalog_table`,
   session-mode `INSERT`, `SELECT`).
3. `SqlEngine::open_with_bace` on a fresh path runs a DDL +
   per-table DML cycle (`CREATE`, `INSERT`, `SELECT`).
4. `DomainSqlEngine::open` after a `_with_bace`-opened engine
   has committed data under a non-Raw profile fails closed
   with a BACE-origin error surface, OR the committed data is
   still all v1 (idempotent-checkpoint rule) — assert
   whichever is actually produced so the test documents
   observed behavior.
5. `SqlEngine::open` fresh is unchanged; the opening itself
   does not fail even on a BACE-written sibling catalog, but
   the first `table_store()` for a BACE-written table fails.
   Document whichever surface actually fires.
6. `DomainSqlEngine::open_with_bace` on a v1-only legacy
   directory succeeds (ADR 0065 §3.5 rule).
7. `SqlEngine::open_with_bace` on a v1-only legacy directory
   succeeds and reads existing rows through the lazy
   `table_store()` path.

## 4. Alternatives considered

**A. Add `MvccSqlEngine::open_with_bace` in the same ADR.**
Rejected: widens scope by two more component stores
(`MvccStore`, `TypedMvccStore`). Separate follow-up.

**B. Store wiring on `DomainSqlEngine` too.** Would allow a
future `DomainSqlEngine::backup_to_with_bace`. Rejected for v1:
operator-facing backup goes through `CatalogLifecycle`.

**C. Make `DomainSqlEngine::open` / `SqlEngine::open` auto-
detect v2 pages and attach a default wiring.** Rejected per
every previous ADR in the series.

**D. Add a BACE-aware DDL path
(`SqlEngine::execute_with_bace`).** Rejected: DDL execution
already runs through the component stores; the wiring is
captured at open time and applies to every commit afterwards.

**E. Expose wiring as a field accessible on the engines.**
Rejected: operators configure wiring through the open call;
they don't need to inspect it afterwards.

## 5. Open design questions (owner review)

1. **Entry-point naming.** Draft: `open_with_bace` +
   `open_with_bace_and_limits`. Matches ADRs 0069/0070.
   Confirm.
2. **`MvccSqlEngine` scope.** Draft: out of scope; follow-up
   ADR. Confirm defer.
3. **G23 DDL coordinator scope.** Draft: out of scope;
   separate follow-up (unblocks ADR 0070 §6 item 6). Confirm
   defer.
4. **Backup entry points on SQL engines?** Draft: NO — none.
   Operators use `CatalogLifecycle::backup_database_to`.
   Confirm.
5. **Shared single wiring vs per-component.** Draft: single.
   Confirm.
6. **Test item 4 / item 5 observed behavior.** Draft: document
   whatever actually fires after a committed mutation under
   each engine, because the ADR 0065 "checkpoint idempotent
   at current LSN" rule means a freshly opened engine with no
   mutations before `open` → `open_with_bace` → immediate
   reopen `open` may legitimately succeed on v1 pages.
   Confirm honesty.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** additive. No on-disk
  format change.

## 7. Dependencies this ADR would add

None. Confined to:

- `crates/bradian-relational/src/typed_table.rs` (`TableStore`)
- `crates/bradian-relational/src/sql.rs` (`SqlEngine`)
- `crates/bradian-relational/src/domain_sql.rs`
  (`DomainSqlEngine`)
- `crates/bradian-relational/tests/` — new tests somewhere
  relevant (likely `shared_domain_sql.rs` and a new or
  existing SqlEngine test file).

No new error variants on any of the SQL engines — every
existing `#[from] StoreError` / component-error plumbing
surfaces the BACE errors.

## 8. What this ADR explicitly does NOT do

- Touch any on-disk format.
- Add new policy keys or codecs.
- Change coordinator backup paths.
- Touch `MvccSqlEngine` / `TypedMvccStore` / `MvccStore`.
- Touch G23 DDL coordinator flows.
- Introduce archive-level compression, encryption, or
  authentication.
- Change per-table directory layout.
- Change the operation-mutex / lock discipline.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0070 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions, especially
   Q2 (defer `MvccSqlEngine`), Q3 (defer G23 DDL
   coordinator), Q4 (no backup entry points on SQL engines),
   Q6 (document observed behavior honestly for items 4/5).
3. Implementation satisfying every item in §3.5.
4. No regressions in `shared_domain_sql*`, `sql*`,
   `domain_sql*`, `typed_table*`, `database_backup.rs`, or
   `backup_restore.rs` test files.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
