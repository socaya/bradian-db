# Bradian SQL subset reference

Normative: [ADR 0055](../adr/0055-sql-subset-conformance-corpus.md). Every
claim below is backed by a passing test in
`crates/bradian-relational/tests/sql_subset_corpus.rs`; a claim without a
cited passing test is not part of this corpus.

Bradian ships three native SQL engines with strict, overlapping subsets:

- **`SqlEngine`** (`sql.rs`) — legacy single-table session.
- **`DomainSqlEngine`** (`domain_sql.rs`) — shared-WAL multi-table session.
- **`HistorySqlEngine`** (`history_sql.rs`) — durable history, expression
  UPDATE, ordered SELECT, savepoints, AS OF, DESCRIBE.

Every object reference **must** be fully qualified: `database.schema.table`.
`execute(...)` accepts a single statement. The native CLI
(`bradian-db sql --execute "..."`) splits on top-level `;` before dispatch.

## 1. Data types

| Type form | Observed `Value` variant | Covering test |
|---|---|---|
| `INT`, `INTEGER`, `INT4` | `Value::Int32(i32)` | `type_int32_and_int64_and_varchar_and_decimal_round_trip` |
| `BIGINT`, `INT8` | `Value::Int64(i64)` | `type_int32_and_int64_and_varchar_and_decimal_round_trip` |
| `BOOLEAN`, `BOOL` | `Value::Boolean(bool)` | `literal_boolean_and_null_case_insensitive` |
| `TEXT` | `Value::String(String)` | `literal_text_empty_and_escaped_and_utf8` |
| `VARCHAR(n)` | `Value::String(String)` (max length enforced at insert) | `type_int32_and_int64_and_varchar_and_decimal_round_trip` |
| `DECIMAL(p,s)` / `NUMERIC(p,s)` | `Value::Decimal(i128)` scaled by column `scale` | `type_int32_and_int64_and_varchar_and_decimal_round_trip` |

`TimestampTz` and `Uuid` are enum variants but are not accepted in literal
syntax on any engine.

## 2. Literal syntax

| Form | Example | Covering test |
|---|---|---|
| Positive integer | `42` | `literal_integer_positive` |
| Negative integer | `-100` | `literal_integer_negative` |
| Text — empty | `''` | `literal_text_empty_and_escaped_and_utf8` |
| Text — escaped single quote | `'it''s ok'` | `literal_text_empty_and_escaped_and_utf8` |
| Text — multi-byte UTF-8 | `'é'` | `literal_text_empty_and_escaped_and_utf8` |
| Boolean (any case) | `TRUE`, `false` | `literal_boolean_and_null_case_insensitive` |
| `NULL` (any case) | `null` | `literal_boolean_and_null_case_insensitive` |

## 3. DDL — `SqlEngine`

| Statement | Covering test |
|---|---|
| `CREATE DATABASE <name>` | `ddl_create_and_insert_select_update_delete_cycle` |
| `CREATE SCHEMA <db>.<schema>` | `ddl_create_and_insert_select_update_delete_cycle` |
| `CREATE TABLE <db>.<schema>.<table> (col type [NOT NULL], PRIMARY KEY (col, ...))` | `ddl_create_and_insert_select_update_delete_cycle` |

## 4. DML across all three engines

| Statement | Constraints | Covering test |
|---|---|---|
| `INSERT INTO <t> (cols) VALUES (lits)` | All declared columns listed; column order may differ from schema | `ddl_create_and_insert_select_update_delete_cycle` |
| `UPDATE <t> SET col=<rhs> [,...] WHERE <pk>=<lit> [AND ...]` | Full PK equality; cannot change PK; `rhs` is literal on `SqlEngine`/`DomainSqlEngine`, expression on `HistorySqlEngine` | `ddl_create_and_insert_select_update_delete_cycle` |
| `DELETE FROM <t> WHERE <pk>=<lit> [AND ...]` | Full PK equality | `ddl_create_and_insert_select_update_delete_cycle` |
| `SELECT col[,...] FROM <t> [WHERE <pk>=<lit> [AND ...]]` | PK equality or full scan | multiple |
| `SELECT * FROM <t>` | — | `ddl_create_and_insert_select_update_delete_cycle` |

## 5. Rejected grammar (all engines)

| Rejection | Covering test |
|---|---|
| SQL comments (`--` and `/* */`) | `rejects_sql_comments_dash_and_block` |
| Multiple statements in one `execute(...)` | `rejects_multi_statement_in_a_single_execute` |
| Partial primary-key predicate | `rejects_partial_primary_key_predicate` |
| `JOIN`, aggregates, `OR`, range `>`/`<`, column aliases, `DISTINCT`, `UNION` | `rejects_join_aggregate_and_or_and_range_and_alias` |

## 6. Transactions — `SqlEngine`

| Statement | Covering test |
|---|---|
| `BEGIN` / `COMMIT` publishes one WAL batch | `sql_engine_session_commit_publishes_single_wal_batch` |
| `ROLLBACK` discards staged changes | `sql_engine_session_rollback_discards_staged_changes` |

## 7. Transactions — `DomainSqlEngine`

| Statement | Covering test |
|---|---|
| `BEGIN` + cross-table DML + `COMMIT` is atomic in the shared WAL | `domain_cross_table_commit_publishes_atomically` |
| `ROLLBACK` discards cross-table changes | `domain_rollback_discards_cross_table_changes` |

## 8. Transactions and sessions — `HistorySqlEngine`

| Statement | Covering test |
|---|---|
| `BEGIN` snapshot session | `history_begin_commit_round_trip` |
| `BEGIN TRANSACTION WITH READ VALIDATION` | `history_begin_with_read_validation_is_accepted` |
| `SAVEPOINT <name>` + `ROLLBACK TO <name>` keeps pre-marker work | `history_savepoint_rollback_to_discards_only_post_marker_work` |
| `RELEASE SAVEPOINT <name>` confirms prior work and allows COMMIT | `release_savepoint_confirms_prior_work_and_allows_commit` |
| `DESCRIBE TABLE <t> AS OF <lsn>` returns the schema at that LSN | `describe_table_as_of_returns_schema_rows` |
| `DESCRIBE TABLE <t>` without `AS OF` refused | `describe_without_as_of_is_rejected` |
| `INSERT INTO ... VALUES (..., DEFAULT)` uses the column default | `insert_default_uses_the_column_default_at_the_target_schema` |

## 9. `UPDATE` expressions — `HistorySqlEngine` only

Grammar (precedence: unary > `*` / `/` > `+` / `-`, parentheses for grouping):

```
assignment := column '=' ( DEFAULT | expression )
expression := sum
sum        := product (('+' | '-') product)*
product    := atom   (('*' | '/') atom)*
atom       := literal | column | ('-' | '+') atom | '(' expression ')'
```

Budgets: ≤128 nodes, ≤32 nesting depth.

| Capability | Covering test |
|---|---|
| `col = col + literal` (BIGINT) | `update_expression_arithmetic_on_bigint` |
| Parentheses override precedence | `update_expression_parentheses_override_precedence` |
| 32-nested-parentheses boundary accepted | `update_expression_boundary_at_32_nested_parentheses_is_accepted` |
| Nesting beyond 32 refused | `update_expression_rejects_nesting_beyond_published_depth` |
| 128-node spend boundary accepted | `update_expression_boundary_at_128_nodes_is_accepted` |
| 129 spends refused | `update_expression_at_129_nodes_is_refused` |
| Arithmetic on TEXT target refused | `update_expression_rejects_arithmetic_on_text_target` |

## 10. Schema evolution — `HistorySqlEngine` only

| Statement | Covering test |
|---|---|
| `ALTER TABLE ... ADD COLUMN ... NULL` after `enable_epoch_row_writes()` | `alter_add_nullable_column_lets_old_rows_read_null_when_epoch_writes_enabled` |
| Migrated G23 history domain refuses `ALTER` without opt-in | `alter_without_epoch_encoding_is_refused_on_a_migrated_history_domain` |
| `DROP COLUMN`, `RENAME COLUMN`, type change refused | `alter_rejects_drop_column_rename_and_type_change` |

## 11. Ordered historical queries — `HistorySqlEngine` only

| Clause | Covering test |
|---|---|
| `ORDER BY col DESC LIMIT n OFFSET m` | `ordered_order_by_limit_offset` |
| `LIMIT` above the configured ceiling is refused | `ordered_rejects_oversized_limit` |

## 12. `AS OF` historical read — `HistorySqlEngine` only

| Behavior | Covering test |
|---|---|
| Returns rows committed at or before the given WAL LSN | `as_of_returns_committed_rows_at_lsn` |
| `AS OF` is refused inside a `WITH READ VALIDATION` session | `as_of_is_rejected_in_a_read_validation_session` |

## 13. Engine feature matrix (observed)

| Feature | `SqlEngine` | `DomainSqlEngine` | `HistorySqlEngine` |
|---|---|---|---|
| CREATE DATABASE/SCHEMA/TABLE | ✅ | ❌ | ❌ |
| ALTER TABLE ADD/DROP NOT NULL | ❌ | ❌ | ✅ (epoch opt-in) |
| INSERT / UPDATE (literals) / DELETE | ✅ | ✅ | ✅ |
| UPDATE with arithmetic expressions | ❌ | ❌ | ✅ |
| SELECT full-PK WHERE and full scan | ✅ | ✅ | ✅ |
| SELECT AS OF | ❌ | ❌ | ✅ |
| ORDER BY / LIMIT / OFFSET | ❌ | ❌ | ✅ |
| SAVEPOINT / ROLLBACK TO | ❌ | ❌ | ✅ |
| Cross-table in one transaction | ❌ | ✅ | ✅ |
| BEGIN WITH READ VALIDATION | ❌ | ❌ | ✅ |

## 14. What this corpus does not cover

- External SQL reference compliance (SQL-92 / SQL-99 / SQL:2016).
- Isolation-level qualification beyond snapshot and opt-in bounded read
  validation. See ADRs 0036, 0037.
- Client wire protocol or driver behaviour — the kernel is accessed via
  Rust APIs and the native CLI only.
- Cross-platform installer, lock-lifecycle and recovery runs on
  Windows / macOS.
- Range ORDER BY cases and `TimestampTz` / `Uuid` sort behaviour are
  exercised by other suites in the crate but are not yet in the
  single-file corpus. `DESCRIBE TABLE AS OF`, `RELEASE SAVEPOINT`,
  `INSERT ... DEFAULT`, the 32-depth nesting boundary, and the
  128-node spend boundary are now in the corpus (§8 and §9 above).

These remain open items under G1-REFERENCE in the fixed roadmap.
