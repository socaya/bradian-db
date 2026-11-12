<p align="left">
  <img src="docs/media/Bradian_Database_Systems_Primary_Horizontal_Blue.svg" alt="Bradian Database Systems — approved primary logo" width="380" />
</p>

# Bradian Database Systems

**A proprietary relational, analytical and hybrid database platform by Vitalworks Limited.**

Bradian Database Systems is being engineered as a native, general-purpose database management system that combines **OLTP**, **OLAP**, and **HTAP** with secure, built-in horizontal scaling, independent distributed installations and browser-based administration through **Bradian Studio**. The product is intended to operate on Linux, Windows, macOS and supported Unix-like systems, from a single-node installation to multi-node clusters and federated deployments.

> **Development status — pre-release.** This repository currently contains an operational analytical-engine foundation, an in-memory relational schema catalog, a portable physical page codec, a **single-process WAL-backed persistent row store**, and **typed, schema-bound table CRUD with primary-key uniqueness checks**, synchronized WAL batches, page checkpoints, a **disk-resident checkpoint B+tree** for indexed lookups and ranges, and a **bounded physical page cache** with generation-aware invalidation, and a **WAL-backed mutable index overlay** for post-checkpoint inserts, updates and deletes, and **crash-safe reconstruction of missing derived B+tree indexes** from validated checkpoint pages. It does **not yet** provide production-grade general SQL, multi-operation ACID/MVCC transactions, a disk B+tree, distributed SQL, automatic sharding, or an independently qualified security stack. Product targets below are requirements, not claims of implemented functionality.

## Product architecture

| Bradian subsystem | Responsibility | Present implementation |
|---|---|---|
| **Bradian Core** | Common types, runtime, catalogs, resource governance, portability | Foundational Rust libraries |
| **Bradian Transaction Engine (OLTP)** | Row storage, SQL writes, constraints, B+tree indexing, ACID/MVCC, WAL and recovery | Catalog, WAL-backed typed rows, and immutable on-disk B+tree with WAL-backed mutable delta and bounded page cache; no SQL/MVCC |
| **Bradian Analytics Engine (OLAP)** | Proprietary columnar storage, vectorized execution, aggregation, optimized queries | Analytical engine foundation |
| **Bradian Hybrid Engine (HTAP)** | Commit-consistent row/column snapshots and combined query planning | Architecture target; not implemented |
| **Bradian Cluster Fabric** | Secure membership, consensus, native routing, sharding and rebalancing | Analytical federation primitives; not a distributed SQL cluster |
| **Bradian Federation** | Controlled synchronization and querying among autonomous installations | Domain-specific foundations |
| **Bradian Security** | Identities, SQL authorization, audit, encryption and cluster trust | Security primitives and **local-only bootstrap API gate** |
| **Bradian Connect** | JDBC/ODBC, data ingestion, migrations, adapters | Source adapters and DHIS2 analytics integration |
| **Bradian Studio** | Web-browser database administration, SQL, cluster operations and telemetry | React analytics administration foundation |

### Architectural principles

Bradian's transactional row representation and committed transaction log are intended to become the **authoritative source of truth**. Its columnar analytical representations will be consistent, versioned projections—not an independent source of transactional truth. Nodes in one strong cluster will have explicit quorum and transaction rules; autonomous installations will use independently defined federation and conflict-resolution semantics. Bradian must not depend on another third-party database server as its primary database engine.

Security, recovery, correctness and operational transparency take precedence over unchecked performance claims. The platform will use well-reviewed cryptographic libraries rather than custom cryptographic algorithms.

## Engineering documentation

The primary specification is **[Bradian Master Architecture and SRS v1.1](docs/Bradian_Master_Architecture_SRS_v1.1.md)**. Start with the [Bradian documentation index](docs/README.md), which links to security requirements, the architecture decision records, implementation gates and historical source material.

| Reference | Purpose |
|---|---|
| [Master Architecture & SRS v1.1](docs/Bradian_Master_Architecture_SRS_v1.1.md) | Product requirements, subsystems, security and implementation programme |
| [Engineering audit and roadmap](docs/implementation/BRADIAN_V1_1_AUDIT.md) | Code-to-SRS gaps and prioritized engineering stages |
| [Relational catalog ADR](docs/adr/0006-generic-relational-catalog.md) | Generic catalog boundary and identifiers |
| [Physical page ADR](docs/adr/0007-portable-physical-page.md) | First native page-envelope specification |
| [Relational catalog gate](docs/gates/BRD-G1-relational-catalog.md) | First relational engineering milestone |
| [Physical page gate](docs/gates/BRD-G2-page-format.md) | Portable page format and outstanding durability requirements |
| [Atomic WAL row-store gate](docs/gates/BRD-G3-wal-row-store.md) | Local WAL-backed row operations and checkpoint recovery |
| [Typed table gate](docs/gates/BRD-G4-typed-table-crud.md) | Persisted typed table CRUD, schema pinning and primary-key checks |
| [Disk B+tree index gate](docs/gates/BRD-G5-checkpoint-btree.md) | Immutable on-disk B+tree, point lookup, range scans and WAL-aware routing |
| [Buffer pool gate](docs/gates/BRD-G6-buffer-pool.md) | Bounded verified physical page cache, LRU eviction and checkpoint generation isolation |
| [Mutable index overlay gate](docs/gates/BRD-G7-mutable-index-delta.md) | WAL-backed live inserts, updates and deletes merged with on-disk checkpoint B+tree |
| [Derived index recovery gate](docs/gates/BRD-G8-index-recovery.md) | Recover missing indexes after interrupted publication; verify physical pointers against authoritative pages |
| [Bootstrap API security](docs/security/BOOTSTRAP_API_AUTH.md) | Temporary local development restrictions |

## Repository organization

```text
bradian-db/
├── crates/
│   ├── bradian-relational/       # Catalog, WAL-backed rows, typed tables, disk B+tree
│   ├── dap-core/                 # Legacy core primitives
│   ├── dap-storage/              # Existing .derc analytical segments, WAL and snapshots
│   ├── dap-runtime/              # Analytical execution operators
│   ├── dap-planner/              # Existing analytics query planning
│   ├── dap-security/             # Security primitives
│   ├── dap-serving/              # Existing analytical HTTP API
│   └── ...                      # Other analytical and DHIS2 integration crates
├── apps/
│   ├── dap-ui/                   # Bradian Studio React application (legacy directory)
│   └── dhis2-integration/        # Optional DHIS2 analytics integration
├── docs/                        # Master SRS, ADRs, gates and security documentation
└── tests/                       # End-to-end integration tests
```

Legacy internal directory and crate names are retained temporarily to protect API, build and storage compatibility. **The product and repository are named Bradian Database Systems / `bradian-db`, not DAP.** Renaming persisted formats or Rust crates requires a documented compatibility migration.

## Local development

### Requirements

- Rust toolchain specified in [rust-toolchain.toml](rust-toolchain.toml)
- Node.js and npm for Bradian Studio
- Compatible development operating system with a writable data directory

### Rust workspace

```sh
cargo check --workspace --locked
cargo test --workspace --locked
```

To test the current relational catalog and physical page codec:

```sh
cargo test -p bradian-relational --locked
```

### Bradian Studio

```sh
cd apps/dap-ui
npm install
npm run dev
```

The browser interface is at the Vite development-server address, normally `http://localhost:5173`. The existing API expects a local Rust process on port 8080. **Protected analytical endpoints require authentication that the current Studio does not yet implement as a browser session.** The UI is not a production-ready secured database management console.

### Local-only analytical API

For development, inject a cryptographically strong administrator token through your secret-management mechanism as `BRADIAN_API_TOKEN` (at least 32 non-whitespace characters), then launch the CLI:

```sh
cargo run -p dap-admin -- serve --root ./data --bind 127.0.0.1:8080
```

Requests to protected endpoints must include `Authorization: Bearer <token>`. The bootstrap API refuses non-loopback binds; it does not provide production TLS, individual login sessions, RBAC, token expiry or data-level authorization. Refer to the [security limitations](docs/security/BOOTSTRAP_API_AUTH.md).

## Branding

The approved Bradian horizontal logo is available in [navy blue](docs/media/Bradian_Database_Systems_Primary_Horizontal_Blue.svg) and [white](docs/media/Bradian_Database_Systems_Primary_Horizontal_White.svg). Bradian Studio uses the approved white mark on its dark navigation and a cropped version of the approved blue artwork for the browser icon. Logo geometry must not be redrawn or restyled.

## Delivery principles

Every milestone must produce tested, executable functionality and traceable requirements. SQL correctness, transaction isolation, crash recovery, secure authorization, cluster safety, and backup restoration are separate release gates. Experimental or in-memory components must never be represented as production-complete.

**Maintainer:** Vitalworks Limited · **Repository:** [socaya/bradian-db](https://github.com/socaya/bradian-db)
