# Bradian Database Systems — Engineering Instructions

**Product:** Bradian Database Systems  
**Canonical repository:** [socaya/bradian-db](https://github.com/socaya/bradian-db)  
**Owner:** Vitalworks Limited  
**Normative requirements:** [Bradian Master Architecture & SRS v1.1](docs/Bradian_Master_Architecture_SRS_v1.1.md)  
**Documentation entrypoint:** [docs/README.md](docs/README.md)

## 1. Product identity and scope

Bradian is a proprietary, portable relational and analytical database management system intended to provide native OLTP, OLAP and unified HTAP, standalone and multi-node operation, secure horizontal scaling, federation and browser-based administration through **Bradian Studio**.

It is **not** merely a DHIS2-specific reporting engine. DHIS2/DER compatibility belongs in optional adapters and the inherited analytical subsystem. All generic relational metadata, SQL types, transaction protocols, cluster membership and security rules must remain independent of the DHIS2 domain.

Use the approved Bradian logos without altering path geometry, proportions, typography or colors. The brand assets are in `docs/media/`. White horizontal artwork is used over the navy navigation in Studio; navy artwork is used on light surfaces. Do not reintroduce the old DAP logo, favicon, or public product name.

## Fixed gate order

Use Master SRS Section 32 Gates 0–9 and [the fixed roadmap](docs/implementation/BRADIAN_MASTER_ROADMAP.md). BRD-G1–BRD-G44 are preserved historical implementation records, not accepted master gates. Do not create another G-number for each code batch. Map changes to the earliest incomplete master gate and its stable task IDs. Maintain the master traceability matrix and run `python3 scripts/verify_roadmap.py`. Gate 1 is active; runtime, local identity and failure/platform qualification remain blockers before moving to Gate 2 work.

## 2. Honest implementation status

Implemented components include the analytical engine, generic WAL-backed relational catalog, typed row persistence, immutable disk B+tree checkpoints, read-only page caching, atomic WAL batches, bounded SQL sessions, shared-domain multi-table writes, opt-in durable historical MVCC/SQL, verified migrations and backup/restore. G35 adds epoch-tagged rows; G36 adds exclusive nullable-column ALTER with catalog/history recovery and schema-aware AS OF; G37 explicitly retires old inactive DDL intents; G38 captures archive receipts and statistics atomically. G39 adds exclusive non-key DROP NOT NULL and implicit NULL for omitted nullable historical SQL INSERT fields. G40 adds immutable typed ADD COLUMN defaults, required additions and historical default projection with version-2 recovery records. G41 adds bounded named savepoints in native durable MVCC and historical SQL, retaining snapshot/conflict checks and requiring rollback after statement errors. G42 adds opt-in bounded point-read validation at commit, including missing keys, read-only transactions and savepoint-preserved observations. G43 adds checked historical UPDATE arithmetic, same-type column references and standalone DEFAULT, with original-row assignment semantics. G44 adds typed historical ORDER BY and bounded LIMIT/OFFSET with logical primary-key tie-breaking. These are bounded single-process implementations. General SQL conformance, serializable isolation, concurrent online DDL, mutable physical page management, production HTAP/distributed SQL, enterprise identity/TLS, drivers and cross-platform recovery qualification remain incomplete. See the fixed master roadmap, gap register and executed verification report for evidence. The Gate 1 native local SQL command exposes a canonical bradian-db executable while preserving dap-admin; private Unix directory checks do not provide full local/enterprise identity.

Do not label a requirement DONE until both implementation and its correctness, security and recovery tests are demonstrated. Record deliverables in `docs/gates/` and explain incomplete functionality accurately in README and UI.

G41 [ADR 0036](docs/adr/0036-bounded-history-transaction-savepoints.md) bounds markers to 16 and copied staged keys to 256 each. Preserve original snapshots and monotone scan-conflict observations. With a marker, a statement error requires ROLLBACK TO/full ROLLBACK before further work or COMMIT; without markers the original whole-transaction abort remains. Savepoints are in-memory and cannot commit independently.

G42 [ADR 0037](docs/adr/0037-bounded-transaction-read-validation.md) keeps read validation explicit: ordinary BEGIN remains snapshot mode. Validate all observed keys under the commit mutex, retain observations across savepoints, and fall back to whole-domain validation after 4096 distinct keys. Validating SQL must reject AS OF reads that bypass its snapshot tracker. This is not the master's fully qualified Serializable isolation level; all participants in a shared invariant must opt in.

G43 [ADR 0038](docs/adr/0038-checked-history-update-expressions.md) scopes expressions to historical exact-PK UPDATE. Preserve the older engines' lexer/grammar, original-row simultaneous assignment semantics, checked numeric bounds and exact fixed-scale decimals. Enforce parser budgets; reject unsupported syntax and comments. Validate the complete result before staging any row mutation.

G44 [ADR 0039](docs/adr/0039-ordered-history-query-windows.md) binds order/window clauses before reads and before projection. Preserve historical-schema binding, explicit NULL direction, logical composite-PK ties and scan conflict observations, including LIMIT 0. Sorting is bounded to 16 keys/10000 input rows; the underlying scan still materializes data. Do not claim streaming, collation or distributed ordering.

## 3. Architecture boundaries

- **Core and relational catalog:** Generic database/schema/table IDs, column types, constraints, dependency analysis and versioned catalog evolution.
- **Transactional engine (future):** Durable row pages, buffer pool, B+tree, WAL, checkpoints, ACID, MVCC, isolation and tested crash recovery.
- **Analytical engine:** Preserve working columnar segment storage, vectorized execution, incremental data products, provenance and DHIS2 analytics contracts.
- **Hybrid engine (future):** Materialize columns only from committed transaction state, using explicit snapshot consistency and freshness.
- **Cluster fabric (future):** Membership, consensus, shard placement, fencing, distributed ACID, automatic rebalancing and built-in routing, not simply an HTTP load balancer.
- **Federation:** Autonomous remote installations with explicit conflict and partition semantics.
- **Security:** End-to-end authorization context, signed identities and mTLS, encryption and key management, tenant isolation, immutable audit and recovery.

No mandatory dependency on an external OLTP/OLAP database server is permitted for the Bradian core. Audited third-party libraries for crypto, networking and compression are acceptable. Never implement custom cryptographic algorithms.

## 4. Compatibility constraints

G40 [ADR 0035](docs/adr/0035-immutable-constant-column-defaults.md) permits immutable typed defaults only on added trailing columns. Preserve empty-map legacy serialization and version-2 intent/activation coupling; existing defaults cannot change.

G39 [ADR 0034](docs/adr/0034-nullability-relaxation-and-insert-null.md) permits only monotone nullability relaxation and nullable trailing additions. Preserve G33's strict additions-only proposal API; the broader transition uses a distinct catalog command. Required/PK INSERT fields without a declared default remain mandatory. Do not introduce SET NOT NULL, backfills or default expressions without their validation/recovery design.

G36/G37 schema recovery is specified in [ADR 0031](docs/adr/0031-recoverable-additive-historical-ddl.md) and [ADR 0032](docs/adr/0032-legacy-ddl-intent-retirement.md). ALTER requires a mutable HistorySqlEngine, G35 encoding and no active row snapshots or catalog proposals. Preserve original pins, validate retained rows against their commit-time schemas and project older additive rows with their immutable declared defaults or NULLs. Recover only exact catalog horizons. Failed live DDL fences row access until coordinated reopen. Legacy inactive intent retirement is explicit and digest-bound; do not silently apply or ignore old fences. G38 [ADR 0033](docs/adr/0033-atomic-history-backup-statistics.md) requires archive receipts and statistics from the same mutation lock.

Native source-consistent backup and staged restore are specified in [ADR 0015](docs/adr/0015-consistent-backup-restore.md) and [BRD-G10](docs/gates/BRD-G10-consistent-backup-restore.md). Capture a single manifest-selected snapshot/index/WAL state under the source mutation mutex. Reject corruption and existing restore destinations; do not claim encrypted, distributed or nonblocking backups.

Checkpoint-generation publication and fail-closed manifest recovery are specified in [ADR 0014](docs/adr/0014-atomic-checkpoint-manifest.md) and [BRD-G9](docs/gates/BRD-G9-checkpoint-publication.md). Always sync authoritative BRPG pages and the derived index before publishing the generation marker; truncate the WAL only after the marker is durable. Do not trust a higher snapshot filename when a committed manifest exists.

G23 introduces `durable_typed::DurableTypedStore`, `history_sql::HistorySqlEngine` and `history_migration::migrate_shared_to_history`; see [ADR 0028](docs/adr/0028-durable-typed-sql-history.md) and [BRD-G23](docs/gates/BRD-G23-durable-typed-sql-history.md). `SELECT ... FROM db.schema.table AS OF <WAL LSN> [WHERE full_pk]` reads retained committed G22 versions. The G16->G23 offline migration copies the latest G16 rows into a NEW history-domain root with verified original catalog/schema/rows; old G16 WAL LSNs are not mapped to G23 historical commits. Fail closed on G18 lifecycle sources, legacy G14 files, conflicting roots and table schema mismatches. Do not auto-migrate or claim general temporal SQL. Older SQL engines refuse history-domain roots.

G22 introduces an independent `durable_mvcc::DurableMvccStore` with the `BRADIAN-DURABLE-MVCC-V1` on-disk namespace; see [ADR 0027](docs/adr/0027-durable-mvcc-history.md) and [BRD-G22](docs/gates/BRD-G22-durable-mvcc-history.md). Each new version/tombstone is a key+commit-LSN record in the SAME synced G3 WAL batch. Explicit durable floor markers precede bounded version deletions, and retained history is queryable after restart. Never claim G22 automatically backs G21 SQL, supports persistent transaction sessions, or shares its physical format with G15/G16 without migration.

G21 adds `typed_mvcc::TypedMvccStore` and `mvcc_sql::MvccSqlEngine`, an opt-in SQL execution path over the G15 shared WAL and G20 in-memory versions; see [ADR 0026](docs/adr/0026-typed-sql-mvcc-integration.md) and [BRD-G21](docs/gates/BRD-G21-typed-sql-mvcc.md). Existing G16 SQL retains its original global-LSN conflict semantics. G21 validates exact catalog schema pins, rejects legacy storage, reconciles G18 first when present and uses the same G16 parser. Overlapping SQL sessions may use per-key MVCC conflict detection, but versions are NOT durably historical and write skew is still possible. Never silently switch engines or claim serializable isolation.

G20 adds a standalone opt-in MVCC row kernel in `crates/bradian-relational/src/mvcc.rs`; see [ADR 0025](docs/adr/0025-mvcc-row-version-foundation.md) and [BRD-G20](docs/gates/BRD-G20-mvcc-foundation.md). It pins in-memory versions by WAL LSN, applies per-key first-committer-wins for non-scanning writes, uses conservative global LSN checks after full scans and WAL-syncs atomic batches before publishing new versions. It DOES NOT yet back G16 SQL, store old versions durably or guarantee serializable isolation. Do not present a G20 restart as historic MVCC snapshot recovery.

G19 implements `CatalogLifecycle::backup_database_to` plus `database_backup::restore_database`; see [ADR 0024](docs/adr/0024-database-wide-consistent-backup.md) and [BRD-G19](docs/gates/BRD-G19-database-wide-backup.md). Hold the coordinator operation mutex and all three catalog/domain/lifecycle RowStore locks while creating the separate verified G10 archives. Check the manifest's component receipts and CRCs before restoring. Reopen the full G18 lifecycle on a temporary root and never publish an unverified or existing destination. CRC is not a signature; do not claim online or distributed backups.

G18 introduces `lifecycle::CatalogLifecycle`, using a separate WAL-backed Pending/Active intent log for schema pin provisioning and restart reconciliation. See [ADR 0023](docs/adr/0023-catalog-domain-lifecycle-reconciliation.md) and [BRD-G18](docs/gates/BRD-G18-catalog-domain-lifecycle.md). Do not claim atomic catalog+domain DDL: only recover interrupted registration of an ALREADY committed catalog table; active missing pins and orphan domain pins are hard errors. Coordinator holds exclusive catalog/domain/lifecycle locks and must release them before opening DomainSqlEngine.

G17 introduces `legacy_migration::migrate_legacy_sql` for a locked, staged, verified OFFLINE migration from G14 per-table WAL directories to a new G15 shared domain; see [ADR 0022](docs/adr/0022-verified-legacy-sql-migration.md) and [BRD-G17](docs/gates/BRD-G17-verified-legacy-migration.md). It does NOT delete source rows or auto-switch clients; G10 catalog backup can checkpoint source metadata. Reject orphan legacy directories, an already-shared source and prior unfinished staging, keep source locks until validated publication, and never treat unfinished staging as a database.

G16 adds an explicit shared-domain SQL frontend in `crates/bradian-relational/src/domain_sql.rs`; see [ADR 0021](docs/adr/0021-shared-domain-sql-integration.md) and [BRD-G16](docs/gates/BRD-G16-shared-domain-sql.md). `DomainSqlEngine` refuses legacy `tables/` data, requires explicit catalog table registration, and executes multi-table SQL DML through one shared-WAL domain. Do not claim automatic migration or catalog+data DDL atomicity.

Cross-table atomic row commits in a **separate shared-WAL storage domain** are implemented in `crates/bradian-relational/src/transaction_domain.rs`; see [ADR 0020](docs/adr/0020-shared-wal-multi-table-atomicity.md) and [BRD-G15](docs/gates/BRD-G15-shared-wal-multi-table.md). All registered tables share one RowStore WAL, and one commit_batch atomically publishes their row mutations. Do not migrate or conflate existing G14 SQL table directories with this new domain; G11 catalog DDL and multi-table SQL BEGIN remain outside G15.

Scoped SQL transaction sessions are defined in [ADR 0019](docs/adr/0019-scoped-sql-transaction-sessions.md) and [BRD-G14](docs/gates/BRD-G14-scoped-sql-transactions.md). `SqlEngine::with_sql_session` accepts SQL BEGIN/COMMIT/ROLLBACK and DML only against one resolved table; any statement error aborts staged work. Do not claim cross-table atomicity, network sessions or MVCC.

Single-table transactions are implemented via `TableStore::begin_transaction` and `SqlEngine::transact_table`; see [ADR 0018](docs/adr/0018-single-table-transactions.md) and [BRD-G13](docs/gates/BRD-G13-single-table-transactions.md). A transaction snapshots one RowStore at a consistent LSN, stages validated changes, rolls back without touching WAL, and commits a bounded batch in one synchronized frame. Do not describe it as SQL BEGIN/COMMIT or multi-table ACID/MVCC.

The first native SQL executor is implemented in `crates/bradian-relational/src/sql.rs`; see [ADR 0017](docs/adr/0017-native-sql-execution.md) and [BRD-G12](docs/gates/BRD-G12-native-sql.md). Preserve strict all-PK predicates and reject unsupported SQL; do not represent these single-row statements as multi-statement ACID transactions.

Catalog metadata persistence is specified in [ADR 0016](docs/adr/0016-persistent-ddl-catalog.md) and [BRD-G11](docs/gates/BRD-G11-persistent-catalog.md). Validated DDL must be durably journaled before publishing the new in-memory catalog. Replay must verify exact event ordering and stable identities. Catalog DDL and independently stored table rows are not yet one transaction. `PersistentCatalog::backup_catalog_to` only backs up its DDL journal; never present this as an entire relational database backup.

The derived index reconstruction and corruption audit are specified in [ADR 0013](docs/adr/0013-derived-index-reconstruction.md) and [BRD-G8](docs/gates/BRD-G8-index-recovery.md). Rebuild only missing indexes from authoritative verified checkpoint pages; never silently replace an existing corrupt published index. A standalone repair bypassing RowStore locking is forbidden.

The incremental WAL-backed mutable index is specified by [ADR 0012](docs/adr/0012-wal-backed-mutable-index.md) and [BRD-G7](docs/gates/BRD-G7-mutable-index-delta.md). It is a synchronized overlay merged with an immutable checkpoint B+tree, **not** in-place physical B+tree writes or a dirty-page manager.

The read-only buffer pool is documented in [ADR 0011](docs/adr/0011-generation-scoped-buffer-pool.md) and [BRD-G6](docs/gates/BRD-G6-buffer-pool.md). Its checked, bounded LRU cache is for immutable snapshot generations only: do not present it as a dirty-page buffer manager, in-place B+tree update manager or MVCC engine.

For the checkpoint B+tree index and WAL-aware lookup, read [ADR 0010](docs/adr/0010-checkpoint-btree.md) and [BRD-G5](docs/gates/BRD-G5-checkpoint-btree.md). This is an immutable disk index over published checkpoints; do not conflate it with in-place B+tree updates, MVCC or a buffer manager.

For typed persistent table CRUD, read [ADR 0009](docs/adr/0009-typed-table-rows.md) and [BRD-G4](docs/gates/BRD-G4-typed-table-crud.md). Secondary unique and foreign-key constraints are rejected until truly enforceable.

For the native row durability protocol, read [ADR 0008](docs/adr/0008-native-row-wal-checkpoint.md) and [BRD-G3](docs/gates/BRD-G3-wal-row-store.md). A single-process WAL-backed row store exists, but cross-platform lock qualification, typed transactional rows, database isolation and cross-platform crash qualification remain unimplemented.

The original analytical components use `dap-*` Rust crate names, `apps/dap-ui/` as an application directory, and `.derc` for the proprietary analytical file format. These **internal legacy identifiers** must not be bulk-renamed until a documented migration and compatibility suite exists. Likewise, keep published DHIS2-specific endpoints wire-compatible unless introducing a versioned replacement.

The canonical GitHub URL, README titles, Studio title and visible brand identity must always be **Bradian Database Systems** or **Bradian Studio**.

## 5. Development process and test evidence

Implement an incremental vertical slice with a small, testable API and clear ownership. Before freezing any persistent format or wire protocol, write an ADR. Add tests for invalid input, privilege boundaries, concurrent operations, partial failure, crash recovery and upgrade compatibility as appropriate.

Use these local checks when requested or at a release gate:

```sh
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cd apps/dap-ui
npm install
npm run typecheck
npm run build
```

**Git workflow:** The owner authorizes publishing all completed, tested Bradian changes directly to the public `socaya/bradian-db` repository's `main` branch on an ongoing basis. Fetch the latest remote state before publishing, reconcile and merge relevant changes without overwriting newer work, and leave `main` current with the completed implementation. This authorization applies to future tested increments; do not request it again for each increment. Automatic GitHub Actions triggers remain disabled; the workflow is manual-only. Do not claim checks have run without actual evidence.

## 6. Security gates

The temporary `BRADIAN_API_TOKEN` is for **loopback-only development** and does not provide a secure browser login. The API must remain inaccessible over an untrusted network until proper TLS/mTLS and per-user authorization are implemented. Never embed secret tokens in JavaScript bundles, files, logs or examples containing live credentials.

SQL authorization must be consistent across row paths, columnar projections, distributed fragments, caches, exports and jobs. Every production security claim needs evidence of isolation, revocation, audit integrity, key rotation, backup protection and incident response.

## 7. Required reading for new work

Start with the [v1.1 master specification](docs/Bradian_Master_Architecture_SRS_v1.1.md), then the [engineering audit](docs/implementation/BRADIAN_V1_1_AUDIT.md), [ADR 0006](docs/adr/0006-generic-relational-catalog.md), [ADR 0007](docs/adr/0007-portable-physical-page.md) and the current milestone gate. The original DAP architectural document and legacy gate files remain in the repository for provenance only; they do not override the Bradian master SRS.
