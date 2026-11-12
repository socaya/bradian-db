# BRD-G12 — Native executable SQL profile

**Specification:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0017](../adr/0017-native-sql-execution.md)  
**Implementation:** `crates/bradian-relational/src/sql.rs`  
**Status:** Working source and integration tests committed to `main`; manual compilation/test acceptance outstanding.

## Implemented capability

A native SQL entrypoint `SqlEngine::open(root).execute(sql)` now tokenizes, parses, binds and executes a strict subset through the durable G11 catalog and G4 typed RowStore, with G5–G9 physical indexing and restart recovery underneath. No external SQL engine is used.

- [x] CREATE DATABASE and CREATE SCHEMA with durable validated catalog metadata.
- [x] CREATE TABLE with declared typed columns, nullability and primary key.
- [x] INSERT with named columns in any order and strict typed validation.
- [x] SELECT projections, SELECT *, complete-primary-key equality lookup and ordered table scan.
- [x] UPDATE non-key columns for an existing primary-key row.
- [x] DELETE by complete primary key.
- [x] Exact integer, Boolean, VARCHAR, TEXT and DECIMAL literals; NULL handling.
- [x] Composite primary-key equality with AND in any component order.
- [x] Reopening the SQL engine recovers previously written catalog and table files.
- [x] Unsupported features and syntax rejected explicitly, including multi-statements.

## Committed regression tests

`crates/bradian-relational/tests/sql_execution.rs` contains six integration tests for end-to-end SQL persistence, unchanged rows after malformed/unsafe SQL, quote escaping and decimal precision, qualification/name resolution, composite key lookup/update/delete and INSERT column reordering.

Source presence is **not** a passing test result. Local Cargo is unavailable in the current execution environment; automatic CI remains disabled per owner instruction.

## Manual acceptance

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Still required before broad SQL/OLTP compliance

- [ ] Manually observed passing compile, tests and lints.
- [ ] Multi-statement ACID transactions, rollback, isolation and MVCC.
- [ ] Atomic catalog/table storage lifecycle and schema migrations.
- [ ] SQL WHERE general predicates, expressions, joins, aggregates and sorting.
- [ ] Durable multi-row INSERT/UPDATE/DELETE and secondary constraint enforcement.
- [ ] Query optimizer, statistics, plan cache, page-level mutation, memory admission.
- [ ] Typed SQL bindings, wire protocol and authenticated client access.
- [ ] Database-wide consistent backups, audit, encryption and full production qualification.

**G12 is a functioning, strictly bounded native SQL executor; not an ANSI SQL engine or a production SQL server.**
