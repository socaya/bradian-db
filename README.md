# Bradian Database Systems (`bradian-db`)

**Status: pre-release analytical prototype, not a completed relational or distributed HTAP database.**

Bradian is being developed as a proprietary cross-platform relational and analytical DBMS with native scaling and Bradian Studio. The inherited DAP Rust workspace already implements portions of the native `.derc` columnar storage, analytical execution, DHIS2 semantic integration and HTTP query service. These modules are retained for compatibility while the authoritative OLTP engine and distributed transaction architecture are developed.

The target specification is [Bradian Master Architecture & SRS v1.1](docs/Bradian_Master_Architecture_SRS_v1.1.md), with the [source audit and staged roadmap](docs/implementation/BRADIAN_V1_1_AUDIT.md).

## Current relational kernel milestones

The independent `bradian-relational` crate now implements a validated **in-memory relational schema catalog** and a **versioned, checksummed 4 KiB portable physical page codec**. The page format is documented in [ADR 0007](docs/adr/0007-portable-physical-page.md), with progress tracked in [G2](docs/gates/BRD-G2-page-format.md).

These are engineering primitives, **not** a durable row store. Row persistence, buffer management, WAL ordering, crash recovery, SQL execution, MVCC, transaction isolation, HTAP integration and distributed scaling remain unimplemented.

## Compatibility

The `dap-*` crate names, source imports, `.derc` signatures and DHIS2-compatible endpoints remain unchanged pending dedicated migration and conformance testing. The canonical GitHub repository is [`socaya/bradian-db`](https://github.com/socaya/bradian-db).

## Build and tests

```sh
cargo check --workspace
cargo test --workspace
cd apps/dap-ui && npm ci || npm install
npm run typecheck
npm run build
```

## Temporary development API protection

Use a securely generated `BRADIAN_API_TOKEN` (at least 32 non-whitespace characters). The bootstrap server requires this token for analytical endpoints and accepts only loopback binds; the public liveness probe is `GET /healthz`. The frontend does not have a login/session workflow yet and therefore cannot use protected endpoints until one is implemented.

See [bootstrap auth limitations](docs/security/BOOTSTRAP_API_AUTH.md). This is **not production-ready** identity management, TLS, RBAC or data isolation.

## Engineering constraints

Do not install another OLTP/OLAP server as Bradian's database core. Reviewed cryptographic, networking and compression libraries are allowed. Never claim full OLTP, HTAP, SQL compliance, horizontal scaling, storage encryption, high availability or production security without implementation and test evidence.
