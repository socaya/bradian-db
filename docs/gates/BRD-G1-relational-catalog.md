# Bradian G1 — Generic relational catalog (first vertical slice)

**Specification:** BDS-MASTER-001 v1.1  
**Status:** Implemented code and tests; CI result required before gate acceptance.

## Delivered

A new `bradian-relational` crate defines domain-neutral database, schema and table objects with stable IDs; a minimal type system; validated primary and unique keys; FK metadata with target validation; namespaced case-insensitive unquoted identifiers; and RESTRICT semantics for dependent drops. These definitions are independent of DHIS2 and leave the historical DAP column-store unchanged.

Tests verify structural behavior, including invalid identifiers and types, key arity, same-database references, primary-key nonnull restrictions, uniqueness, and dependency rules.

## Not delivered

This is **not** a complete relational DBMS. Rows are not stored; SQL cannot yet create or query the catalog; PK/FK semantics are not enforced against data; metadata is in memory only. MVCC, transactional DDL, WAL and crash recovery are open requirements.

## Gate sign-off

- [ ] `cargo check --workspace --locked` passes
- [ ] `cargo test --workspace --locked` passes, including relational catalog tests
- [ ] `cargo clippy --workspace --all-targets --locked -- -D warnings` passes
- [ ] Existing analytical and DHIS2 tests remain green
- [ ] Catalog persistence and SQL semantic ADRs are assigned

Do not mark this gate complete until all checks pass.
