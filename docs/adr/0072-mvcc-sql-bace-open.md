# ADR 0072 — MvccSqlEngine BACE open (implementation; **ACCEPTED**)

**Status: ACCEPTED, 15 November 2026.** Owner accepted with the
drafted defaults; implementation landed in the same commit.

The last SQL engine without BACE opens is now wired. All four
SQL engines — `SqlEngine`, `DomainSqlEngine`, `HistorySqlEngine`,
`MvccSqlEngine` — accept `BaceWiring`.

Implementation summary:

- `crates/bradian-relational/src/mvcc.rs` —
  `MvccStore::open_with_bace` + `_with_bace_and_limits`.
  Shared `open_inner` delegates to `RowStore::open_with_bace`
  when wiring is present; in-memory history seeding unchanged.
- `crates/bradian-relational/src/typed_mvcc.rs` —
  `TypedMvccStore::open_with_bace` + `_with_bace_and_limits`.
  Shared `open_inner` delegates to
  `MvccStore::open_with_bace_and_limits`.
- `crates/bradian-relational/src/mvcc_sql.rs` —
  `MvccSqlEngine::open_with_bace` + `_with_bace_and_limits`.
  G18 lifecycle pre-flight uses
  `CatalogLifecycle::open_with_bace` when wiring is supplied.
  No wiring stored on the engine.
- `crates/bradian-relational/tests/mvcc_sql.rs` — 3 new tests.

973 workspace tests pass; clippy clean on
`bradian-relational --all-targets`.

## 1. Context

- `MvccSqlEngine::open_with_limits` opens
  `PersistentCatalog::open_with_limits` and
  `TypedMvccStore::open_with_limits`. The typed store opens
  `MvccStore::open_with_limits`, which opens
  `RowStore::open(root)` — the legacy pre-BACE entry point.
- `PersistentCatalog::open_with_bace` already ships
  (ADR 0069). The missing rungs are
  `MvccStore::open_with_bace` and
  `TypedMvccStore::open_with_bace`, plus the
  `MvccSqlEngine::open_with_bace` entry point.
- G21 is an opt-in **in-memory** MVCC engine. Each
  `MvccStore` owns one exclusive `RowStore` and seeds an
  in-memory version map at open time; historical versions are
  not durable (ADR 0025). BACE wiring affects only the
  underlying RowStore's checkpoint/recovery byte format; the
  in-memory history semantics are unchanged.
- G21's opt-in `MvccSqlEngine` pre-flight also reconciles any
  G18 lifecycle journal present (lines 74–79 of mvcc_sql.rs).
  If the operator has a G19 coordinator next to a G21 engine
  root, the temporary `CatalogLifecycle::open` call holds and
  releases three component locks before this engine opens its
  own. For BACE, the pre-flight will use
  `CatalogLifecycle::open_with_bace` when wiring is supplied.

## 2. Scope of this ADR

**In scope:**

- `MvccStore::open_with_bace` + `_with_bace_and_limits`.
- `TypedMvccStore::open_with_bace` + `_with_bace_and_limits`.
- `MvccSqlEngine::open_with_bace` + `_with_bace_and_limits`.
- The G18 lifecycle pre-flight inside
  `MvccSqlEngine::open_with_bace` uses
  `CatalogLifecycle::open_with_bace` when wiring is supplied,
  matching the format of the subsequent eager opens.
- All three new entry points take a single shared `BaceWiring`
  cloned into each component store (matches ADR 0069/0070/0071
  shared-wiring model).
- Tests covering the §3.5 checklist.

**Explicitly out of scope** (each needs its own ADR):

- **G23 DDL coordinator BACE**
  (`HistorySqlEngine::enable_epoch_row_writes`,
  `execute_ddl`, `activate_schema`). Still deferred;
  separate ADR.
- **Authenticated envelope (`BACA`).** Different family of
  work; separate ADR.
- **`.derc` carrier BACE.** Analytical column segments;
  orthogonal carrier; separate ADR.
- **BRWL carrier BACE.** WAL frames; separate ADR.
- **BACE-aware backup entry points on `MvccSqlEngine`.** G21
  has no durable history and no coordinator backup; operators
  who want a full-database backup use
  `CatalogLifecycle::backup_database_to` (ADR 0069) against
  the underlying G19 root.
- **Changing the pre-existing legacy-rejection pre-flights.**
  The `history-domain` refusal and the `tables/` refusal
  stay intact under both open paths.
- **Per-component wirings.** Draft uses the shared single
  wiring, matching every prior ADR in the series.
- **Changing in-memory MVCC semantics.** Everything remains
  per the G20/G21 ADRs (0025, 0026).

## 3. Proposed design

### 3.1 `MvccStore::open_with_bace`

```rust
impl MvccStore {
    pub fn open_with_bace(root, wiring) -> MvccResult<Self>;
    pub fn open_with_bace_and_limits(root, wiring, limits) -> MvccResult<Self>;
    fn open_inner(root, limits, wiring: Option<BaceWiring>) -> MvccResult<Self>;
}
```

The only mutation inside is `RowStore::open(root)` →
`RowStore::open_with_bace(root, _, wiring)` when wiring is
present. The in-memory history seed afterwards is unchanged.

### 3.2 `TypedMvccStore::open_with_bace`

```rust
impl TypedMvccStore {
    pub fn open_with_bace(domain_root, catalog_tables, wiring);
    pub fn open_with_bace_and_limits(domain_root, catalog_tables, wiring, mvcc_limits);
    fn open_inner(domain_root, catalog_tables, mvcc_limits, wiring: Option<BaceWiring>);
}
```

Delegates to `MvccStore::open_with_bace` when wiring is present.

### 3.3 `MvccSqlEngine::open_with_bace`

```rust
impl MvccSqlEngine {
    pub fn open_with_bace(root, wiring) -> SqlResult<Self>;
    pub fn open_with_bace_and_limits(root, wiring, mvcc_limits, catalog_limits)
        -> SqlResult<Self>;
    fn open_inner(root, mvcc_limits, catalog_limits, wiring: Option<BaceWiring>);
}
```

No wiring stored on the engine (G21 does not expose backup;
nothing opens lazily after `open*`). The pre-flight lifecycle
reconcile (when `root/lifecycle` exists) uses
`CatalogLifecycle::open_with_bace(root, wiring.clone())` when
wiring is supplied, so the temporary coordinator stops on
BACE-wrapped state consistently with the subsequent eager
opens.

### 3.4 Legacy compatibility invariants

- `MvccSqlEngine::open` and the matching legacy variants are
  unchanged.
- `MvccSqlEngine::open_with_bace` tolerates v1-only
  directories per ADR 0065 §3.5 (inherited transitively).
- `MvccSqlEngine::open` on a directory whose components have
  v2 pages fails closed through the existing
  `PersistentCatalogError::Storage` / `DomainError::Storage`
  surfaces, carrying `BaceOpenWithoutBaceWiring`.

### 3.5 Compliance obligations (implementation test checklist)

1. `MvccStore::open_with_bace` on a fresh path opens and
   round-trips a put/get across a commit.
2. `TypedMvccStore::open_with_bace_and_limits` on a
   provisioned shared-domain root reads its schema pins.
3. `MvccSqlEngine::open_with_bace` on a provisioned G15/G18
   root runs a SELECT and an MVCC transaction (INSERT inside
   a session) successfully.
4. `MvccSqlEngine::open_with_bace` on a v1-only legacy root
   succeeds and reads existing rows (ADR 0065 §3.5
   tolerance).
5. `MvccSqlEngine::open_with_bace` on a root whose G18
   lifecycle journal has an interrupted pin invokes
   `CatalogLifecycle::open_with_bace` for the reconcile
   (observed via a run that would fail on legacy open after
   the BACE wiring is forced — or, honestly, documented and
   covered transitively by the ADR 0069 test suite).

Items 6/7 from the earlier ADR 0070/0071 pattern (legacy
`open` on BACE-written root fails closed; actual v2 pages
produced through this engine) are **not required here** for
the same reason recorded in ADR 0070 §6 and ADR 0071 §3.5
items 4/5: the "checkpoint idempotent at current LSN" rule
means fresh-open → `_with_bace` → immediate reopen produces
no v2 pages through this engine. The RowStore-layer
fail-closed invariant is covered by ADR 0067's test suite and
inherited transitively.

## 4. Alternatives considered

**A. Skip `MvccSqlEngine` BACE entirely.** Rejected: leaves
one SQL engine inconsistent with the other three, and
operators using the opt-in G21 path would have no way to
reach BACE-wrapped storage.

**B. Store wiring on `MvccSqlEngine` for a future backup
API.** Rejected: G21 has no durable history and no
coordinator backup. Backup is a G19 concern via
`CatalogLifecycle`.

**C. Auto-detect v2 pages on legacy open.** Rejected per
every prior ADR in the series.

**D. Share the `TypedMvccStore::open_with_bace` body with the
G23 `DurableTypedStore::open_with_bace` body.** Rejected:
different physical stores (`MvccStore` vs
`DurableMvccStore`); the structural similarity is
coincidental.

## 5. Open design questions (owner review)

1. **Entry-point naming.** Draft: `open_with_bace` +
   `open_with_bace_and_limits`. Confirm.
2. **G18 lifecycle pre-flight under BACE.** Draft: use
   `CatalogLifecycle::open_with_bace(root, wiring.clone())`
   when wiring is supplied. Alternative: always use legacy
   `CatalogLifecycle::open`. Confirm the consistent-wiring
   stance.
3. **Store wiring on the engine?** Draft: NO. Confirm.
4. **Shared single wiring vs per-component.** Draft: single.
5. **Test item 5 (G18 reconcile path).** Draft: documented
   with a note that the behavior is inherited from ADR 0069
   test suite, not re-tested here. Confirm defer.

## 6. Impact on invariants

- **INV-001 commit correctness:** no change.
- **INV-002 one authoritative state:** no change.
- **INV-003 no authorization gap:** no change.
- **INV-004 no unsafe automatic availability:** N/A.
- **INV-005 recoverable lifecycle:** additive; no on-disk
  format change.

## 7. Dependencies this ADR would add

None. Confined to:

- `crates/bradian-relational/src/mvcc.rs` (`MvccStore`)
- `crates/bradian-relational/src/typed_mvcc.rs`
  (`TypedMvccStore`)
- `crates/bradian-relational/src/mvcc_sql.rs`
  (`MvccSqlEngine`)
- `crates/bradian-relational/tests/mvcc_sql.rs` — new tests

No new error variants.

## 8. What this ADR explicitly does NOT do

- Touch any on-disk format.
- Add new policy keys or codecs.
- Add backup paths on MvccSqlEngine.
- Touch G23 DDL coordinator flows.
- Touch `.derc` or BRWL carriers.
- Introduce archive-level compression, encryption, or
  authentication.
- Change in-memory MVCC version semantics.

## 9. Acceptance evidence required before marking Accepted

1. ADRs 0060–0071 accepted — **satisfied**.
2. Owner sign-off on §5 open design questions.
3. Implementation satisfying items 1–4 in §3.5; item 5
   documented as inherited.
4. No regressions in `mvcc_sql.rs`, `shared_domain_sql.rs`,
   `sql_execution.rs`, `domain_sql.rs`, `database_backup.rs`,
   or `backup_restore.rs` test files.

Only then can this ADR move from **PROPOSED** to **ACCEPTED**.
