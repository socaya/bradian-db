# BRD-G18 — Durable catalog/domain lifecycle reconciliation

**Specification:** Bradian Database Systems Master Architecture and SRS v1.1  
**Design:** [ADR 0023](../adr/0023-catalog-domain-lifecycle-reconciliation.md)  
**Implementation:** `crates/bradian-relational/src/lifecycle.rs`  
**Status:** Source and integration tests committed to `main`; compilation, test execution and power-loss qualification outstanding.

## Implemented

- [x] Separate native WAL-backed lifecycle journal with explicit format version.
- [x] Exact-schema table identity records in `Pending` and `Active` states.
- [x] `CatalogLifecycle::provision_existing(TableId)` requires committed G11 metadata.
- [x] Synchronize pending intent, then idempotent G15 domain pin, then active acknowledgement.
- [x] Automatic restart reconciliation of interrupted pending registration.
- [x] Idempotent retry after a crash before or after the physical schema pin.
- [x] Validate every pinned domain schema against the committed catalog.
- [x] Refuse orphan pins, missing catalog identities and mismatched schemas.
- [x] Refuse previously active table records whose domain pin is missing.
- [x] Adopt older G16 registered pins only when the catalog definition matches.
- [x] Reject malformed journal marker, unrecognized key namespace and invalid records.
- [x] Refuse nonempty legacy table directories until G17 migration.
- [x] Maintain exclusive file locks over catalog, shared domain and lifecycle journal.
- [x] Support G16 SQL after the coordinator completes and releases ownership.

These checkmarks represent code and test coverage, **not passing test execution**.

## Tests and manual release acceptance

`crates/bradian-relational/tests/catalog_lifecycle.rs` contains 11 cases including interrupted intent replay, a full cross-table SQL roundtrip after provisioning, legacy pin adoption, corrupted/missing metadata rejection, and competing ownership.

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Require fault injection at three WAL boundaries, disk-full/permission failures, restart while registration is incomplete, corrupt schema pin testing, source/database lock ownership verification, and performance of large catalogs.

## Still open

- [ ] Atomic G11 CREATE TABLE and G15 domain registration in a **single** cross-store commit.
- [ ] Transactional DROP and ALTER TABLE including stored-row/index lifecycle.
- [ ] Unified catalog/domain/lifecycle consistent backup and restore.
- [ ] Full SQL data-definition grammar with durable schema migrations.
- [ ] MVCC, versioned schema pins and concurrent DDL/DML isolation.
- [ ] Network SQL sessions and cluster-wide metadata consensus.

**G18 establishes recoverable registration and consistency checking, not database-wide transactional DDL.** Automatic CI remains disabled as requested.
