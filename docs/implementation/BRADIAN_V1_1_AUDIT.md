# Bradian Database Systems v1.1 — source audit and implementation roadmap

**Baseline:** `docs/Bradian_Master_Architecture_SRS_v1.1.md`. **Reviewed source:** the original Rust DAP analytical workspace. This document reports implementation status, not a completed DBMS.

## Updated implementation status (12 November 2026)

The Bradian relational crate now includes a WAL-backed generic relational metadata catalog and bounded native SQL executor and local single-table atomic transactions and scoped SQL BEGIN/COMMIT/ROLLBACK sessions and a separately provisioned shared-WAL multi-table atomic row transaction domain with an explicitly registered multi-table SQL execution frontend and verified offline legacy SQL-to-shared-domain migration and WAL-backed catalog/domain provisioning reconciliation, a versioned physical page codec and a real **single-process WAL-backed row store** with inserts, replacements, deletes, atomic multi-key WAL batches, ordered scans, page checkpoints, immutable disk B+tree point/range lookup, bounded read-only page cache, WAL-backed mutable logical index overlay, missing-index reconstruction from verified checkpoint pages, CRC-protected authoritative checkpoint generation publication, verified local snapshot-plus-WAL backup/restore and restart recovery. A mutable ordered delta merges WAL-committed changes with the immutable disk index without introducing in-place dirty pages. The physical page cache is generation-scoped, retains checksum-verified pages and is invalidated on checkpoint replacement. The B+tree is derived from completed checkpoints and does not implement mutable page updates or buffer management. See [ADR 0008](../adr/0008-native-row-wal-checkpoint.md) and [G3](../gates/BRD-G3-wal-row-store.md). The row store remains a bounded independent subsystem; it does **not** provide complete relational SQL, distributed transactions, MVCC, on-disk B+trees, multi-process file locking or enterprise recovery qualification. Automatic CI remains disabled by owner request.

## Original audit baseline

The repository contains a proprietary analytical column format (`.derc`), WAL-backed analytical partitions, vectorized operators, DHIS2 semantic modules, an Axum analytical API, a React analytics administration interface, and test suites. It does **not** contain a verified independent general-purpose authoritative relational database with complete SQL DDL/DML, B+tree row pages, ACID/MVCC transactions, cross-shard atomic commits, distributed consensus, or strongly consistent HTAP projections.

Existing DAP traceability DONE statuses refer to analytical requirements only; they do not fulfill Bradian v1.1 transactional and enterprise requirements.

## Phase 0: secure and rename product-facing identity

Preserve legacy `dap-*` crates, Rust imports, `.derc` on-disk identifiers, DHIS2 wire compatibility and previously persisted objects. Present the product as **Bradian Database Systems**, CLI as `bradian-db`, and administration as **Bradian Studio**. Introduce a fail-closed, **loopback-only** bootstrap API administrator token. This is *not* full end-user authorization. Native TLS/mTLS, scoped principals and Studio login are release blockers.

**Proof required:** all protected API paths reject missing/invalid tokens; liveness remains public; full workspace tests, Rust lint and frontend typecheck pass before merge. Always re-test integrations because unauthenticated DHIS2 clients are intentionally rejected until their credentials are configured.

## Phase 1: native relational kernel (P0)

Add a generic versioned database/schema/table catalog separate from DHIS2 UID semantics. Specify portable row page layout, page IDs, B+tree indexes, transaction identities, stable SQL types, write-ahead log recovery, buffer management and constraints. Implement `CREATE TABLE`, `INSERT`, `SELECT`, `UPDATE`, `DELETE`, `BEGIN`, `COMMIT`, `ROLLBACK`, primary/foreign keys and tested crash durability as incremental executable slices. Design formats and ADRs before writing bytes.

## Phase 2: ACID/MVCC and SQL correctness (P0)

Implement transaction visibility, isolation and locking, uniqueness races, phantom/write-skew detection under claimed isolation, savepoints, schema migrations and SQL conformance fixtures. Require concurrent schedule testing and differential result comparisons, not just CRUD smoke tests.

## Phase 3: authoritative HTAP (P0)

Only committed transactional state can publish columnar projections. Define commit-position, transaction-consistent row/column snapshots, delta replay, staleness modes and permission propagation. Reuse the existing analytical executor and segment code where correctness remains valid.

## Phase 4: distributed SQL and auto scaling (P1)

Implement trusted cluster membership, replicated log and quorums, shard placement, leader fencing, multi-gateway routing, online splitting/rebalancing and durable cross-shard commit/recovery. Keep autonomous geographic federations separate from strong clusters. Fault-inject network partitions, duplicated RPCs and coordinator failure.

## Phase 5: high-assurance security and operations (release blockers)

Implement identity provider, MFA-capable Studio sessions, SQL-level per-principal authorization, tenant separation, storage/WAL/backup encryption, managed key lifecycle, TLS/mTLS, privileged audit and incident response. Add cluster-consistent PITR, restore rehearsal, driver conformance and Windows/Linux/macOS CI before production qualification.

## Compatibility and security notes

The original analytics-only API and file formats are not to be bulk-renamed. Existing analytical namespaces are transitional until explicit migration tests. The bootstrap token environment variable is `BRADIAN_API_TOKEN`, and its presence cannot justify deployment on a public network. Database-admin permissions, direct query authorization, token revocation/expiry, browser login and secure internal service credentials are **not implemented** by the bootstrap layer.

## Verification commands

```sh
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps/dap-ui
npm ci || npm install
npm run typecheck
npm run build
```
