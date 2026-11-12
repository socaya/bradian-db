# ADR 0006 — Generic relational catalog boundary

**Status:** Accepted for first in-memory catalog milestone, pending durable catalog ADR  
**Source:** Bradian Master Architecture & SRS v1.1, §3 and §23-26.

## Decision

Introduce the independent `bradian-relational` Rust crate with explicit `DatabaseId`, `SchemaId` and `TableId`. No DHIS2 UIDs or analytical period assumptions enter this crate. The catalog owns logical namespace and constraint *definitions*, while future storage and transaction services own persistence, concurrent data writes and actual integrity enforcement.

In this early, deliberately narrow SQL profile, identifiers are unquoted ASCII, 1–63 bytes, and case-insensitive. They are folded to lowercase and must start with a letter or underscore. This restriction is **temporary**, and full SQL quoted identifiers, Unicode normalization, collations and compatibility rules must be addressed before claiming SQL standard compliance.

A table has validated typed columns, one nonempty primary-key definition, zero or more unique-key definitions and foreign keys referring to stable table IDs. PK columns cannot be nullable. Foreign keys must reference an existing PK or nonnullable unique key of the same database, with identical type and column arity. Dropping an object that retains dependencies is rejected. This is *catalog-level structural validation*, not runtime enforcement of data integrity.

## Implementation

Use deterministic `BTreeMap` object collections with monotonically increasing identity values, scoped name uniqueness and validated construction APIs. No externally reachable mutable fields are exposed. Metadata definitions remain transient because crash-safe DDL, schema version leases, WAL catalog records and atomic snapshots require their own thoroughly tested design.

## Consequences and later work

- New logical relational metadata does not alter `.derc`, current analytical partitions or DHIS2 adapters.
- The first milestone does **not** support SQL parsing/execution, row storage, table data, MVCC, transactions, catalog persistence, online schema migrations, tenant policies or cluster metadata consensus.
- The next ADR must select a versioned persistent catalog representation and recovery protocol; do not serialize these Rust structs directly as an unversioned permanent file format.
- Future FK enforcement must coordinate with transaction isolation, including uniqueness races and concurrent delete/insert operations.
- Composite unique keys currently require explicit nonnullable target columns for FK references; NULL-compatible SQL uniqueness and deferrable constraints remain separate features.

## Acceptance criteria

`cargo test -p bradian-relational` must demonstrate scoped namespaces, case folding, invalid-name and invalid-type rejection, PK constraints, FK type/target validation, cross-database prevention, stable object identities and dependency-safe drops. `cargo check --workspace --locked` and existing analytical tests must remain green.
