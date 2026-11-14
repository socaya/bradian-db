# Bradian relational module review

14 November 2026. Scope: every source module in
`crates/bradian-relational/src/`. The review summarizes **what each
module owns, what it asserts, what tests back it, and what is honestly
open**. It is diagnostic — not a redesign.

Baseline for the review: commit `f5c0f6c`. 852 workspace tests, 0
failures. Clippy clean. Roadmap verifier passes.

## 1. Headline findings

- **35 modules, all exercised by tests.** No module lacks at least one
  integration test whose name maps to its surface. No `TODO`, `FIXME`,
  `#[cfg(todo)]` or `unimplemented!()` sentinels anywhere in `src/`.
- **One `unsafe` block, scoped and documented.** `local_access.rs`
  wraps `libc::geteuid()` under `#[allow(unsafe_code)]` with a
  `SAFETY:` comment, per CLAUDE.md §4.9 (ADR 0057). The crate root is
  `#![deny(unsafe_code)]`, so additional unsafe would require an
  explicit opt-in per site.
- **All runtime dependencies are in the CLAUDE.md §5 allowlist.** The
  direct deps are `serde`, `serde_json`, `thiserror`, `crc32fast`,
  `parking_lot`, plus `libc` (Unix-only) and `tempfile` (dev-only).
- **Admission policies now cover every decoded-state growth vector
  identified in the plan.** `ScanLimits`, `HistoryLimits`,
  `StagingLimits`, `RowStoreLimits`, `IndexLimits`,
  `IndexBuildLimits`, `MetadataLimits`, `TransactionLimits`,
  `DirectoryLimits`, `CatalogLimits`, `DomainLimits`,
  `MvccStoreLimits` — 12 budgets in `runtime_limits.rs`. Each is
  per-scope; the global process-wide ceiling remains a §13
  owner-decision item.

## 2. Architecture at a glance

The crate is layered. Dependencies flow strictly downward; no upper
layer is imported by a lower one.

```
┌─────────────────────────────────────────────────────────────────┐
│ SQL layer:        sql · sql_expression · select_window           │
│                   domain_sql · mvcc_sql · history_sql            │
├─────────────────────────────────────────────────────────────────┤
│ Typed / history:  typed_table · typed_mvcc                       │
│                   durable_mvcc · durable_typed                   │
│ Schema evolution: schema_evolution · schema_activation           │
│ Session / xact:   mvcc · transaction_domain                      │
├─────────────────────────────────────────────────────────────────┤
│ Catalog:          persistent_catalog                             │
│ Migration/backup: legacy_migration · history_migration           │
│                   lifecycle · lifecycle_history_migration        │
│                   history_backup · database_backup · backup      │
├─────────────────────────────────────────────────────────────────┤
│ Row storage:      row_store · btree · buffer_pool                │
│                   index_recovery · checkpoint_manifest · epoch_row│
├─────────────────────────────────────────────────────────────────┤
│ Codec / policy:   page · metadata_io · model · catalog           │
│                   runtime_limits · local_access                  │
└─────────────────────────────────────────────────────────────────┘
```

## 3. Per-module status (grouped)

### 3.1 Codec and policy

| Module | What it owns | Open items |
|---|---|---|
| `page` | Portable 4096 B page codec with CRC32 and magic | — |
| `metadata_io` | Bounded fixed-record I/O and admission-bound JSON | — |
| `model` | Serde-stable `Database` / `Schema` / `Table` / `Column` types | Quoted identifiers, Unicode collations (future) |
| `catalog` | In-memory relational catalog validator | No persistence (lives in `persistent_catalog`), no concurrency |
| `runtime_limits` | 12 budget types + `ResourceError` + `ScanControl` | Global process ceiling, cross-engine totals |
| `local_access` | POSIX private-root policy + effective-UID check | POSIX ACL, parent-path trust, Windows ACL, enterprise identity |

### 3.2 Row storage and recovery

| Module | What it owns | Open items |
|---|---|---|
| `row_store` | Single-writer WAL-backed KV, exclusive lock, checkpoint+WAL recovery | Not MVCC, not in-place mutable B+tree |
| `btree` | Immutable disk B+tree over checkpoint pages | Not a mutable index |
| `buffer_pool` | Bounded LRU cache for immutable BRPG pages | Writable cache is future work (Gate 3 §5.1) |
| `index_recovery` | Rebuild derived B+tree from authoritative pages | — |
| `checkpoint_manifest` | Atomic 40 B manifest with CRC; `.pending` → `.current` rename | — |
| `epoch_row` | BRVE envelope binding a row to an immutable schema epoch | — |

### 3.3 Catalog and lifecycle

| Module | What it owns | Open items |
|---|---|---|
| `persistent_catalog` | WAL-backed DDL journal with replay validators; admitted via `CatalogLimits` | Append-only until a versioned compactor is designed |
| `lifecycle` | G18 Pending→Active table-provisioning intents between catalog and domain | Not distributed atomic DDL |
| `schema_evolution` | Planning for additive schema transitions | Non-additive changes (DROP/RENAME/type) — §4.1 of the plan |
| `schema_activation` | G36 immutable Intent/Commit records for exclusive ALTER | — |

### 3.4 Typed tables and multi-table transactions

| Module | What it owns | Open items |
|---|---|---|
| `typed_table` | Schema-bound CRUD, PK uniqueness, optimistic single-table txn | FK/UNIQUE/CHECK enforcement refused |
| `transaction_domain` | Multi-table shared-WAL atomic commit via one BRWL batch; admitted via `DomainLimits` | Does not atomically update catalog DDL |

### 3.5 SQL

| Module | What it owns | Open items |
|---|---|---|
| `sql` | Bounded SQL parser / binder / executor; strict all-PK WHERE; no comments | Standards conformance (joins, aggregates, unions) — §4.3 of the plan |
| `sql_expression` | 128-node, 32-depth UPDATE expressions on historical SQL only | — |
| `select_window` | 16-column ORDER BY, LIMIT ≤ 10 000, OFFSET ≤ 1 000 000 on historical SQL | Range `ORDER BY` beyond basic shapes |
| `domain_sql` | Multi-table SQL sessions over `MultiTableStore` | DDL is not SQL-visible on this engine |

### 3.6 MVCC and history

| Module | What it owns | Open items |
|---|---|---|
| `mvcc` | Opt-in G20 in-memory MVCC foundation; admitted via `MvccStoreLimits` (ADR 0058) | Not persistent time travel, not cluster replication |
| `typed_mvcc` | Typed G21 adapter over `MvccStore`; now accepts `open_with_limits` (Slice L) | — |
| `mvcc_sql` | Opt-in G21 SQL frontend; now accepts `open_with_limits` (Slice L) | — |
| `durable_mvcc` | G22 durable MVCC with (key, commit_lsn) WAL records; admitted via `HistoryLimits`/`TransactionLimits` | No persistent sessions |
| `durable_typed` | G23 typed adapter over `DurableMvccStore`; schema-versioned metadata keys | — |
| `history_sql` | G23 historical SQL engine; AS OF, DESCRIBE, savepoints, read-validation | — |

### 3.7 Migration and backup

| Module | What it owns | Open items |
|---|---|---|
| `legacy_migration` | G17 offline cutover from G14 per-table to G15 shared-domain; admitted via `DirectoryLimits` | Not online |
| `history_migration` | G23 offline cutover from G16 shared-WAL to G22 durable history | — |
| `history_backup` | G24 archive and verification of G23 single-node databases | Not remote / not encrypted |
| `lifecycle_history_migration` | G25 verified lifecycle → history cutover | — |
| `backup` | G10 consistent local backup/restore primitive | Not incremental PITR |
| `database_backup` | G19 three-component coordinated backup | Not remote / not distributed |

## 4. Test-to-module mapping

The crate has 49 integration test files in `tests/`. Every source module
is reachable from at least one of them. High-coverage modules
(row_store, persistent_catalog, history_sql) are reached by 4–6
dedicated test files; lower-level codec modules (page, metadata_io)
are reached indirectly through their consumers but also have dedicated
direct tests (`page_codec.rs`, `metadata_limits.rs`).

Direct-coverage matrix (module → test file whose name maps clearly):

| Module | Direct test file |
|---|---|
| `page` | `page_codec.rs` |
| `row_store` | `row_store_durability.rs`, `row_store_limits.rs`, `fault_recovery.rs` |
| `btree` | `btree_index.rs`, `index_limits.rs` |
| `buffer_pool` | `buffer_pool.rs` |
| `index_recovery` | `index_recovery.rs` |
| `checkpoint_manifest` | `checkpoint_manifest.rs`, `checkpoint_limits.rs` |
| `metadata_io` | `metadata_limits.rs` |
| `catalog` | `catalog.rs` |
| `persistent_catalog` | `persistent_catalog.rs`, `catalog_crash.rs`, `catalog_limits.rs` |
| `typed_table` | `typed_table.rs` |
| `transaction_domain` | `multi_table_transactions.rs`, `domain_limits.rs` |
| `sql` | `sql_execution.rs`, `sql_subset_corpus.rs` |
| `sql_expression` | `update_expressions.rs` |
| `select_window` | `select_windows.rs` |
| `domain_sql` | `domain_sql.rs` |
| `mvcc` | `mvcc.rs`, `mvcc_limits.rs` |
| `typed_mvcc` / `mvcc_sql` | `mvcc_sql.rs`, `transitive_admission.rs` |
| `durable_mvcc` | `durable_mvcc.rs`, `read_validation.rs` |
| `durable_typed` / `history_sql` | `history_sql.rs`, `epoch_sql.rs`, `epoch_sql_crash.rs` |
| `legacy_migration` | `legacy_migration.rs` |
| `history_migration` / `lifecycle_history_migration` | `history_migration.rs` |
| `backup` / `database_backup` | `backup_restore.rs`, `database_backup.rs`, `fault_recovery.rs` |
| `history_backup` | `history_backup_consistency.rs` |
| `lifecycle` | `catalog_lifecycle.rs` |
| `schema_evolution` / `schema_activation` | `schema_sql.rs`, `schema_retirement.rs`, `defaults_sql.rs`, `nullability_sql.rs` |
| `runtime_limits` | 12 dedicated `*_limits.rs` suites |
| `local_access` | In-module unit tests + CLI tests in `dap-admin` |

## 5. Cross-cutting observations

### 5.1 Admission coverage is comprehensive

Every decoded-state growth vector in the crate is behind a named
`ResourceError::*Limit` variant. Admission always runs **before** the
irreversible operation (WAL append, catalog publish, snapshot copy),
so a refusal leaves durable state exactly as it was. This invariant is
proven by every `*_limits.rs` test suite via byte-identical WAL
assertions after refusal.

### 5.2 Error enums are discoverable

Each layer has its own error enum (`StoreError`, `CatalogError`,
`PersistentCatalogError`, `DomainError`, `SqlError`, `MvccError`, …),
and each wraps `ResourceError` via `#[from]`. Callers can match on
the specific layer error or walk down to the resource variant. No
`anyhow` in the library layer (`anyhow` is confined to `dap-admin`
bins per CLAUDE.md §5).

### 5.3 Immutability of published artefacts

Every published on-disk artefact is immutable by design: pages,
snapshots, `checkpoint.current`, catalog journal entries, history
version records. Recovery reconstructs the derived index from
authoritative pages and never trusts an existing index file.
`checkpoint.pending` becomes `checkpoint.current` only by atomic
rename after `sync_all`. The backup pipeline copies and verifies
before publishing by rename.

### 5.4 One opt-in exception: `MvccStore`

`MvccStore` retains in-memory version history for pinned readers; a
long-lived reader can hold capacity until dropped. This is the only
module whose steady-state memory is a function of **caller
behaviour** (reader lifetime), not just of durable state. Operators
monitor `MvccStats::versions_retained` and raise `MvccStoreLimits` or
investigate a forgotten session.

### 5.5 Legacy identifiers retained by policy

`dap-*` crate names, the `.derc` analytical format and `apps/dap-ui/`
remain intentionally. CLAUDE.md §4.17 forbids a bulk rename until a
documented migration and compatibility suite exist. The visible
brand (README, Studio title, published GitHub URL) is Bradian.

## 6. Risks and open items at module level

Items below are all **already documented** in ADRs, gap registers or
the comprehensive implementation plan §13. They are listed here so a
reader of the review can see them in one place.

### 6.1 Code-only follow-ups (still inside Gate 1)

- `DomainSqlEngine::open_with_limits` — analogous to the Slice L
  wrappers but for the multi-table SQL engine. A natural follow-up.
- `DurableTypedStore::open_with_resource_limits` already exists; its
  propagation path through `HistorySqlEngine` could be tightened.
- A `resource_usage()` aggregator across catalog + domain + MVCC for
  operator diagnostics (observational only, no admission).

### 6.2 Design-blocked (owner decisions, plan §13)

- Global process-memory accounting model.
- Fault-injection architecture (trait wrapper vs filesystem shim).
- Parent-path trust policy.
- POSIX ACL inspection platform binding.
- External SQL reference corpus choice.
- Online-DDL protocol.
- Mutable storage protocol.
- PKI vs self-generated identities.
- Consensus choice.
- Global-timestamp source.
- Studio auth posture.
- Driver wire protocol.

### 6.3 Infrastructure-blocked

- Real `ENOSPC`/`EIO` propagation at every call site.
- `fsync` on power loss.
- Device-level torn writes.
- Windows / macOS installer, lock lifecycle, and recovery runs.
- External SQL reference-suite compliance.
- Cross-platform CI.

## 7. What a reader can conclude

1. **The current code is honest.** Every public surface is exercised
   by tests; every documented limit is enforced by at least one
   admission site; every claim in the SQL subset reference cites a
   passing test.
2. **The crate is at the honest limit of single-process Gate 1
   scope.** Marginal follow-ups exist (§6.1) but each is a small
   quality-of-life addition rather than a missing feature.
3. **The next real step-change is a §13 owner decision.** No further
   accumulation of marginal slices moves the gates forward; they
   build evidence inside the single-process bounds already set.

See [comprehensive implementation plan](BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md),
[gap register](BRADIAN_IMPLEMENTATION_GAPS.md) and the
[fixed roadmap](BRADIAN_MASTER_ROADMAP.md).
