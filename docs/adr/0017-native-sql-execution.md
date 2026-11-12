# ADR 0017 — Native SQL parsing, binding and single-row execution

**Bradian Database Systems · Master Architecture & SRS v1.1**  
**Status:** Working source and integration tests committed to `main`; manual compilation and correctness qualification outstanding.  
**Dependencies:** [ADR 0009 — typed persistent rows](0009-typed-table-rows.md), [ADR 0016 — durable catalog](0016-persistent-ddl-catalog.md), [ADR 0012 — WAL index delta](0012-wal-backed-mutable-index.md).

## Purpose and architectural fit

Bradian's G11 implementation records durable catalog changes, while its G4 row engine supports WAL-backed typed tables and validated primary-key operations. G12 introduces an actual SQL entrypoint that binds those subsystems together. It does not substitute an external SQL database or fabricate results: supported SQL is tokenized, parsed, resolved against the recovered catalog, converted to typed values and executed through native `TableStore` and `PersistentCatalog` APIs.

The new `bradian-relational::sql::SqlEngine` opens a native database root, keeps the durable catalog in `root/catalog/`, and places physical table stores at `root/tables/<stable_table_id>/`. Table paths derive from persisted numeric catalog IDs rather than user-controlled names. The engine serializes statements through a `parking_lot` mutex; the catalog and each table store retain their existing storage locking, WAL synchronization and strict decoding.

## Supported statement profile

The G12 profile deliberately requires three-part `database.schema.table` identifiers and uses unquoted ASCII catalog identifiers. SQL keyword comparisons are case-insensitive. A single optional trailing semicolon is accepted. A multi-statement string is **rejected** rather than partially executed.

| Statement | Supported form | Execution |
|---|---|---|
| CREATE DATABASE | `CREATE DATABASE billing` | Persist a validated database definition through the G11 DDL WAL journal |
| CREATE SCHEMA | `CREATE SCHEMA billing.public` | Persist the schema under its resolved database |
| CREATE TABLE | `CREATE TABLE billing.public.invoices (id BIGINT NOT NULL, amount DECIMAL(12,2) NOT NULL, PRIMARY KEY(id))` | Validate and persist typed table metadata; physical table directory is provisioned on first use |
| INSERT | `INSERT INTO billing.public.invoices (id,amount) VALUES (1,12.50)` | Convert literals to typed values; perform one non-upserting WAL-backed INSERT with uniqueness checking |
| SELECT | `SELECT id,amount FROM billing.public.invoices WHERE id=1` | Resolve projection columns; use native indexed PK lookup and return typed rows |
| SELECT * | `SELECT * FROM billing.public.invoices` | Validate and scan all stored rows; no arbitrary SQL filtering |
| UPDATE | `UPDATE billing.public.invoices SET amount=15.00 WHERE id=1` | Fetch a single primary-key row, validate new typed values, persist its replacement |
| DELETE | `DELETE FROM billing.public.invoices WHERE id=1` | Delete a single row by complete primary key |

WHERE is limited to equality predicates on **all** primary-key columns. Multiple key components may be joined with `AND` in any order. A WHERE clause that references non-key columns, omits a key component or uses another operator is rejected. UPDATE cannot modify primary-key columns.

The supported CREATE TABLE scalar types are `INT/INTEGER/INT4`, `BIGINT/INT8`, `BOOLEAN/BOOL`, `TEXT`, `VARCHAR(n)`, and `DECIMAL(p,s)/NUMERIC(p,s)`. Other catalog-supported types such as UUID and timestamp remain available through the Rust typed-table API but are not yet part of G12's SQL literal grammar.

## Lexer, parser and binding rules

The native lexer recognizes unquoted identifier tokens, signed decimal or integer literals, SQL single-quoted strings with doubled apostrophe escapes, Boolean constants, NULL, and the limited symbols required for the grammar. It rejects unterminated strings, unexpected characters and oversized token streams.

The parser validates the entire statement before executing a command. It rejects trailing tokens, multiple statements, unsupported syntax and SQL features beyond this profile. Name resolution uses a consistent owned snapshot of G11's persistent metadata catalog. Missing databases, schemas, tables and columns are errors, never empty success results.

INSERT requires every declared column to be named exactly once. Column order in the statement can differ from physical table order. Before the WAL is touched, the executor verifies value types, nullable rules, decimal precision and scale, and string bounds. `DECIMAL` input becomes an exact unscaled `i128`, not a floating-point approximation. SELECT returns the explicitly selected columns and typed `Value` vectors in `SqlResultSet::Rows`; writes return `SqlResultSet::Command { affected }`.

## Storage durability and correctness

The catalog is persisted through G11's validated DDL journal and the RowStore WAL. An INSERT, UPDATE or DELETE is executed through G4's typed table layer, which validates row values and uses the existing WAL-backed primary-key commitment protocol. Reopening `SqlEngine` reconstructs the catalog, resolves stable table IDs and recovers matching table files and their synchronized WAL mutations.

To avoid a false transactional guarantee, G12 does **not** expose `BEGIN`, `COMMIT`, `ROLLBACK`, multi-statement execution, or a multi-row UPDATE/DELETE predicate. It also does not support JOIN, GROUP BY, ORDER BY, aggregates, expression evaluation, secondary indexes, subqueries, SQL networking or database drivers. These require explicit future engineering gates.

One important limitation is that G11 catalog DDL and creation of the separate G4 table storage files are not in a common database-wide transaction. G12 therefore records `CREATE TABLE` in the catalog and lazily initializes the physical table on first use. In a storage fault scenario, catalog creation can succeed while first physical table opening later fails. It must not be represented as atomic catalog+table data provisioning. The SQL engine's mutex serializes only statements handled by the **same engine instance**; it is not MVCC, interprocess transactional isolation or a network concurrency protocol.

## Tests and acceptance

The `tests/sql_execution.rs` integration suite covers create/insert/select/update/delete with restart persistence, unchanged rows after rejected SQL, exact decimal arithmetic, escaped quote literals, NULL and NOT NULL enforcement, strict qualified name resolution, composite key lookup in arbitrary predicate order, and column-order independence of INSERT.

**Verification status:** Source files and tests have been committed directly to GitHub `main`, but Cargo compilation, test execution and Clippy results have not yet been independently verified. Owner-requested CI remains disabled.

Manual checks:

```sh
cargo check --workspace --locked
cargo test -p bradian-relational --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Native query-planning optimizations, SQL authorization, multi-statement ACID, MVCC and crash-safe multi-object DDL remain mandatory before production SQL claims.
