# BRD-G16 — Multi-table SQL execution over a shared WAL

**Normative requirements:** Bradian Database Systems Master Architecture & SRS v1.1  
**Architecture:** [ADR 0021](../adr/0021-shared-domain-sql-integration.md)  
**Implementation:** `crates/bradian-relational/src/domain_sql.rs`  
**Status:** Rust implementation and integration tests committed on `main`; compilation and manual test acceptance still required.

## Implemented

- [x] Separate `DomainSqlEngine` with the durable catalog and G15 shared-WAL transaction store.
- [x] Reuse G12's existing SQL lexical, parsing and typed binding rules rather than duplicating its grammar.
- [x] Explicit `register_catalog_table` resolves persisted catalog identities and durably pins table schemas.
- [x] Refuse roots containing any legacy per-table storage until a verified migration is implemented.
- [x] Scoped `with_session` supports SQL BEGIN/COMMIT/ROLLBACK.
- [x] SQL INSERT, SELECT, UPDATE and DELETE across multiple registered domain tables.
- [x] Cross-table staged changes are visible to the same transaction before commit.
- [x] One commit invokes G15's single synchronized BRWL batch for all changed tables.
- [x] Invalid statements abort the entire staged transaction; session closure drops uncommitted work.
- [x] Unsupported DDL, nested BEGIN, multi-statement input and unregistered tables fail closed.
- [x] Structured `SqlError::Domain` retains domain-level failures, including conflict detection.
- [x] Eight end-to-end SQL integration tests covering all above scenarios and restart.

Checkmarks indicate **code and test cases present**, not passing test execution.

## Acceptance commands

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Production qualification additionally requires crash/power-loss fault injection during WAL sync, concurrent registration and DML stress, backup/restore with catalog consistency, and cross-platform advisory-lock verification.

## Open architectural requirements

- [ ] Verified offline or online migration from G14 independent table WAL directories into the G15 shared domain.
- [ ] Atomic catalog DDL and row-data transaction/lifecycle coordination.
- [ ] Persistent independent client sessions and SQL wire protocol.
- [ ] MVCC and SQL isolation-level configuration.
- [ ] Foreign keys, secondary unique indexes, constraints and general SQL predicates.
- [ ] Query optimization, joins, aggregation and range operators.
- [ ] Cluster coordination and database-wide consistent backup.

**G16 provides cross-table atomic SQL DML only in the explicitly provisioned shared domain.** The older SQL engine is intentionally not altered to silently adopt these guarantees. Automatic CI stays disabled.
