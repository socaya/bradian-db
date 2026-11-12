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

## 2. Honest implementation status

The repository currently includes analytical columnar storage (`.derc`), analytical operators, DHIS2 adapters, an analytical API, a React Studio foundation, a generic **in-memory** relational schema catalog, and a versioned physical page **codec**, and single-process WAL-backed key/value row persistence, immutable disk B+tree checkpoints with atomic manifest publication, verified backup/restore, and missing-index reconstruction, WAL-backed mutable logical index deltas, generation-scoped read-only page caching, and typed table CRUD with primary-key checks with synchronized writes, atomic multi-key batch commits, multi-page snapshots and restart recovery. A production database engine with mutable WAL-protected B+tree pages and buffer management, full SQL execution, persistent catalog, multi-operation ACID/MVCC transactions, strongly consistent HTAP, self-managing distributed SQL cluster, enterprise identity/TLS, and certified recovery are **not yet implemented**.

Do not label a requirement DONE until both implementation and its correctness, security and recovery tests are demonstrated. Record deliverables in `docs/gates/` and explain incomplete functionality accurately in README and UI.

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

Native source-consistent backup and staged restore are specified in [ADR 0015](docs/adr/0015-consistent-backup-restore.md) and [BRD-G10](docs/gates/BRD-G10-consistent-backup-restore.md). Capture a single manifest-selected snapshot/index/WAL state under the source mutation mutex. Reject corruption and existing restore destinations; do not claim encrypted, distributed or nonblocking backups.

Checkpoint-generation publication and fail-closed manifest recovery are specified in [ADR 0014](docs/adr/0014-atomic-checkpoint-manifest.md) and [BRD-G9](docs/gates/BRD-G9-checkpoint-publication.md). Always sync authoritative BRPG pages and the derived index before publishing the generation marker; truncate the WAL only after the marker is durable. Do not trust a higher snapshot filename when a committed manifest exists.

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

**Git workflow:** Changes may be committed directly to `main` under the current owner instruction. Automatic GitHub Actions triggers have been disabled for now; the workflow remains available for manual invocation. Do not claim checks have run unless there is actual evidence.

## 6. Security gates

The temporary `BRADIAN_API_TOKEN` is for **loopback-only development** and does not provide a secure browser login. The API must remain inaccessible over an untrusted network until proper TLS/mTLS and per-user authorization are implemented. Never embed secret tokens in JavaScript bundles, files, logs or examples containing live credentials.

SQL authorization must be consistent across row paths, columnar projections, distributed fragments, caches, exports and jobs. Every production security claim needs evidence of isolation, revocation, audit integrity, key rotation, backup protection and incident response.

## 7. Required reading for new work

Start with the [v1.1 master specification](docs/Bradian_Master_Architecture_SRS_v1.1.md), then the [engineering audit](docs/implementation/BRADIAN_V1_1_AUDIT.md), [ADR 0006](docs/adr/0006-generic-relational-catalog.md), [ADR 0007](docs/adr/0007-portable-physical-page.md) and the current milestone gate. The original DAP architectural document and legacy gate files remain in the repository for provenance only; they do not override the Bradian master SRS.
