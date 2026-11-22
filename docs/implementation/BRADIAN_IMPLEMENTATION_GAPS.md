# Bradian implementation gap register

Updated 13 November 2026. Scope and evidence refer to the checked implementation,
not blanket completion of the master SRS. Automatic CI remains manual-only.

## Fixed roadmap and earliest remaining work

Use [Master Gates 0–9](BRADIAN_MASTER_ROADMAP.md). Gate 0 evidence now includes the
complete requirement matrix, source/format inventory and threat model. Open global
ordering and authorization prototypes remain explicit blockers before their formats
are frozen. Gate 1 is the active kernel implementation gate, not master Gate 44.

| Master gate/task | Remaining gap | Required next evidence |
|---|---|---|
| 1 / G1-MEMORY | Query/history/staging, backing/recovery maps, build/metadata buffers, CLI lines, per-domain historical SQL/fallible-native admission, row-store + legacy-migration directory listings, decoded persistent-catalog state, decoded multi-table-domain state and in-memory MvccStore versions are controlled; ADR 0110 adds `WorkspaceUsage::total_bytes()`/`total_rows()` + opt-in `GlobalBudget`; ADR 0170 wires automatic engine-admission of the global budget into every SQL engine; ADR 0171 exposes `DurableTypedStore` retained-history keys/versions/bytes through `WorkspaceUsage::with_durable_typed`; ADR 0172 adds `CacheStats::cached_bytes()`, `RowStore::page_cache_bytes()` and `WorkspaceUsage::with_row_store_caches(..)` so ADR 0011 read-only page-cache bytes fold into `GlobalBudget::check` totals; automatic HistorySqlEngine-held sampling and memory-pressure/failure qualification remain open | Memory-pressure/failure qualification (infrastructure); see [latest evidence](BRADIAN_GATE1_PAGE_CACHE_USAGE_VERIFICATION_2026_11_21.md) |
| 1 / G1-LOCAL-ACCESS | Shared `bradian_relational::local_access` enforces POSIX private-root invariants (symlink refusal, mode 0o700, non-directory refusal, effective-UID ownership, bounded create-if-missing) across the native SQL CLI and future library callers; ADR 0109 adds POSIX parent-path trust (`ParentPathPolicy::{AllowSticky, Strict, Ignore}`); POSIX ACLs, Windows ACL and enterprise identity remain unqualified | POSIX ACL inspection, Windows ACL and enterprise identity; see [latest evidence](BRADIAN_GATE1_LOCAL_ACCESS_UID_VERIFICATION_2026_11_14.md) |
| 1 / G1-FAULTS | Zero-padded WAL extension, zero-filled CRC body, `.tmp` orphans at higher LSNs, stray incidental files, leftover pending manifests and POSIX ENOSPC/EFBIG propagation (ADR 0111) are covered; fsync-on-power-loss, device-level torn writes and Windows/macOS qualification remain open | Fsync-on-power-loss rig, device-level torn-write test lab and platform recovery runs; see [latest evidence](BRADIAN_GATE1_FAULT_RECOVERY_VERIFICATION_2026_11_14.md) |
| 1 / G1-REFERENCE | A single-file positive/negative SQL subset corpus is published (`docs/reference/BRADIAN_SQL_SUBSET.md`) and each claim is backed by a passing test (ADR 0112 extension adds 9 new rejected-grammar + boundary rows, 46 tests total); external SQL-92/99/2016 reference-suite compliance, full documented isolation qualification and Windows/macOS installer/lock tests remain open | External SQL reference-suite compliance, installation qualification; see [latest evidence](BRADIAN_GATE1_SQL_SUBSET_CORPUS_VERIFICATION_2026_11_14.md) |

The canonical local SQL command and Linux development installer close only the
G1-CLI entry-point task. They do not accept Gate 1. New commits stay within the
current master gate until its exit conditions pass.

## Verified implementation tasks (historical record IDs)

| Gap | Implemented behavior | Evidence |
|---|---|---|
| Epoch codec disconnected from SQL | Explicit G35 capability transition; mixed BRRW/BRVE reads and writes | [G35](../gates/BRD-G35-live-schema-epoch-rows.md), twelve tests |
| Catalog revision cannot activate against history | Exclusive nullable-column ALTER with a durable cross-WAL decision | [G36](../gates/BRD-G36-recoverable-historical-alter.md) |
| Historical rows bound only to current schema | Immutable schema timeline, commit-time validation and NULL projection | G36 SQL and corruption tests |
| AS OF columns resolved against latest schema | SELECT * / named-column binding and DESCRIBE use the requested schema epoch | G36 historical query tests |
| Interrupted catalog/history activation unrecoverable | Exact-state roll-forward at all four durable boundaries, owner fencing and idempotent reopen | G36 recovery tests and sixteen torn-frame cases |
| Old inactive intent protocol blocks upgrade | Explicit digest-bound retirement preserving archival records and catalog horizon | [G37](../gates/BRD-G37-legacy-ddl-retirement.md), six tests |
| Backup manifest samples later source statistics | Receipt and statistics captured under one mutation lock | [G38](../gates/BRD-G38-consistent-history-backup-metadata.md), two tests |
| Restore did not enforce the DDL catalog horizon | Full G30/G36 horizon validation before publication | History backup verifier and schema restore tests |
| Non-key NOT NULL relaxation unavailable | Checked DROP NOT NULL through G36 recovery, with earlier schema/value preservation | [G39](../gates/BRD-G39-nullability-relaxation.md) |
| Every INSERT field required after nullable additions | Omitted nullable fields receive NULL; missing required fields still abort the SQL batch | G39 insertion and rollback tests |
| Constant defaults unavailable for added columns | Immutable typed defaults, required additions, old-row projection and INSERT DEFAULT | [G40](../gates/BRD-G40-immutable-column-defaults.md), recovery and compatibility tests |
| Transaction cannot recover part of staged work | Bounded SAVEPOINT, ROLLBACK TO and RELEASE with failed-state recovery and preserved conflict checks | [G41](../gates/BRD-G41-history-savepoints.md), native, SQL and process-exit tests |
| Point reads are never validated at commit | Opt-in bounded read validation, including absent keys, scans and savepoint observations | [G42](../gates/BRD-G42-transaction-read-validation.md), concurrent and twenty-schedule tests |
| Historical UPDATE accepts literals only | Bounded checked arithmetic, same-type column references and standalone DEFAULT, evaluated from the original row | [G43](../gates/BRD-G43-checked-update-expressions.md), numeric corpus and transaction tests |
| Historical SELECT cannot order or page results | Typed ORDER BY, explicit NULL placement and bounded LIMIT/OFFSET with deterministic PK ties | [G44](../gates/BRD-G44-ordered-query-windows.md), paging, conflict and restore tests |
| JSON SQL round-trip lost declared type | `typed_table::Value::Json(String)` variant + tag 8 encoding; `coerce` emits `Value::Json`; backward-compat decode for pre-ADR-0173 tag-5 rows; BRADIAN_SQL_SUBSET.md §1 updated | [ADR 0173](../adr/0173-value-json-variant.md), 7 round-trip tests in `tests/json_value_type.rs` |
| Audit log not wired into authz-gated SQL path | `SqlEngine::execute_as_audited(authz, sql, &AuditChain)` appends BLAKE3-chained `{timestamp, principal, sql, outcome}` on every call; preserves ADR 0098 fast-fail-on-denial; new `SqlError::Audit` surfaces chain-append failures (no silent gap) | [ADR 0174](../adr/0174-execute-as-audited.md), 7 success/denial/error/tamper/persistence tests in `tests/sql_engine_audited.rs` |
| COLLATE clause unreachable from CREATE TABLE | Parser accepts `COLLATE binary\|nocase` after column nullability; catalog rejects COLLATE on non-text columns; ORDER BY uses the declared per-column collation (PK tie-break stays binary) | [ADR 0175](../adr/0175-collate-clause-create-table.md), 6 parser + ordering tests in `tests/collation_sql.rs` |
| UNIQUE constraint declared but never enforced | Parser accepts `UNIQUE (col, ...)` at CREATE TABLE; `typed_table::validate_table` no longer refuses UNIQUE; **every SQL engine (SqlEngine, DomainSqlEngine, HistorySqlEngine, MvccSqlEngine)** enforces at INSERT + UPDATE via `UniqueConstraintChecker` against a full-table scan (UPDATE self-filters by PK) and returns `SqlError::Unique` on violation | [ADR 0176](../adr/0176-unique-enforcement-sql-insert.md) + [ADR 0177](../adr/0177-unique-enforcement-sql-update.md) + [ADR 0178](../adr/0178-unique-enforcement-domain-sql.md) + [ADR 0179](../adr/0179-unique-enforcement-history-mvcc.md), 8+8+5+5+4 tests across `tests/unique_enforcement_sql.rs`, `tests/unique_update_sql.rs`, `tests/unique_domain_sql.rs`, `tests/unique_history_sql.rs`, `tests/unique_mvcc_sql.rs` |
| TIMESTAMPTZ accepted only integer-micros literals | `coerce` parses ISO 8601 `YYYY-MM-DDTHH:MM:SS[.ffffff][Z|±HH:MM]` into UTC micros via Hinnant days-from-civil; exact microsecond precision; integer path preserved | [ADR 0180](../adr/0180-iso8601-timestamp-literals.md), 7 round-trip / offset / rejection tests in `tests/iso8601_timestamp.rs` |
| Reserved words could not be used as user identifiers | Lexer accepts `"name"` quoted-identifier tokens with `""` escape; parser lowercases at the boundary to preserve ADR 0006 catalog semantics; keyword matching never binds to quoted forms | [ADR 0181](../adr/0181-quoted-identifiers.md), 7 lex / round-trip / rejection tests in `tests/quoted_identifiers.rs` |
| CHECK column constraint unparsed and unenforced | Parser accepts `col TYPE ... CHECK (<op> <literal>)` (ops `=,<>,<,<=,>,>=`); catalog persists `column_checks`; shared `check_row` helper runs at INSERT + UPDATE on every engine; NULL skips per SQL three-valued logic; new `SqlError::Check { column, op, literal }` surfaces violations | [ADR 0182](../adr/0182-column-check-constraint.md), 7 accept/reject/update/multi-clause/null/type-at-create/restart tests in `tests/check_constraint_sql.rs` |
| Prepared statements accepted only positional `$N` | `PreparedStatement.named_params` + `scan_named_placeholders` + `bind_named_literals`; validation enforces per-name presence/absence/identifier rules; `Driver::execute_prepared` binds positional then named; mixed `$N` + `:name` in one template works; serde `skip_if_empty` preserves legacy wire bytes | [ADR 0183](../adr/0183-named-prepared-parameters.md), 10 bind / validate / mix / rejection / serde tests in `tests/named_prepared.rs` |
| FOREIGN KEY unenforced; action keywords unparsed | Parser accepts `FOREIGN KEY (c, ...) REFERENCES <qual> (p, ...) [ON DELETE action] [ON UPDATE action]`; catalog `add_foreign_key` resolves + validates (arity, types, PK/non-null-UNIQUE target, SET-NULL nullable-child rule); `validate_table` relaxed; SqlEngine INSERT + UPDATE run `foreign_key_check` (parent-exists, NULL FK disables); DELETE applies RESTRICT / CASCADE / SET NULL / NO ACTION per the stored `on_delete`; transitive CASCADE respects downstream RESTRICT; new `SqlError::ForeignKey { constraint, reason }` with `reason` ∈ `"missing-parent"` / `"restrict"` | [ADR 0184](../adr/0184-foreign-key-restrict.md) + [ADR 0185](../adr/0185-foreign-key-actions.md), 10 + 8 tests in `tests/foreign_key_sql.rs` + `tests/foreign_key_actions_sql.rs` |
| JSON path operators unparsed | Lexer emits `Token::Arrow` / `Token::ArrowText`; SELECT projection accepts `col -> 'key'` (returns `Value::Json`) and `col ->> 'key'` (returns `Value::String` with JSON strings unwrapped); missing key / NULL cell → `Value::Null`; non-JSON cell → `SqlError::Type`; output column labels render as `col->'key'` / `col->>'key'` | [ADR 0186](../adr/0186-json-path-operators.md), 8 extraction / missing-key / null / non-JSON / numeric-key tests in `tests/json_path_sql.rs` |
| FK enforcement only on `SqlEngine` | New `TxFkRowAccess` trait + `fk_check_child_row_tx` / `fk_apply_parent_delete_tx` helpers in `sql.rs`; impls for `MultiTableTransaction`, `DurableTypedTransaction`, `TypedMvccTransaction`; wired at INSERT + UPDATE + DELETE on `DomainSqlEngine`, `HistorySqlEngine`, `MvccSqlEngine` so FK semantics are identical across every engine; pending parent inserts in the same tx are visible to child FK checks | [ADR 0187](../adr/0187-foreign-key-session-engines.md), 11 insert / delete / cascade / set-null / tx-visibility tests across the three engines in `tests/foreign_key_session_engines.rs` |

## Remaining implementation and qualification

| Priority | Gap | Required next evidence |
|---|---|---|
| P0 | Broader schema evolution | Default expressions and SET/DROP DEFAULT, SET NOT NULL/backfill, DROP/RENAME/type changes, dependency planning and recovery tests |
| P0 | Concurrent online DDL | Schema leases/reader fencing and explicit old-writer commit semantics; current ALTER requires exclusive mutable ownership |
| P0 | SQL conformance and transaction semantics | Joins, general predicates, expression ordering/collations and general expressions outside the G43 UPDATE subset, secondary/foreign-key enforcement, full isolation-level qualification and reference schedules; ordinary snapshot mode still permits write skew, while G42 validates reads only when opted in |
| P0 | Enterprise access and transport | Individual sessions, SQL authorization, revocation, tenant isolation and TLS/mTLS; current bootstrap gate is loopback-only |
| P0 | Operational recovery qualification | Real power-loss, disk-full, torn-device writes, Windows/macOS tests and long-running restore rehearsals |
| P1 | Mutable physical storage | WAL-protected dirty pages and buffer management; existing on-disk B+tree checkpoints remain immutable |
| P1 | HTAP | Commit-consistent column projections and row/column snapshot/freshness semantics |
| P1 | Strong distributed SQL | Consensus, shard ownership, fencing, distributed transactions and partition/rebalancing tests |
| P1 | Client and Studio integration | Service/wire protocol for relational sessions, drivers and a secured administration workflow; Rust APIs are not a public SQL server |
| P1 | Recovery/metadata growth | Schema history compaction with reference proofs, remote/PITR/incremental archives and workload benchmarks |

[Current executed verification](BRADIAN_GATE1_SINGLE_TABLE_LIMITS_VERIFICATION_2026_11_13.md) records the test
scope and limits. These open requirements must not be presented as production-ready
features or silently enabled through fallback storage engines.
