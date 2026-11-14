# Bradian operations guides

This directory collects runtime-governor documentation for operators.
Every entry describes **a shipped admission policy or executable surface**;
items without an executed verification report do not appear here. For
the normative design behind each policy see the cited ADR; for the
executed-evidence trail see the cited verification report.

## Admission policies with full operations guides

| Guide | ADR | Scope |
|---|---|---|
| [HISTORY_QUERY_LIMITS](HISTORY_QUERY_LIMITS.md) | [0041](../adr/0041-bounded-historical-query-scans.md) | Scan input rows / bytes / work budgets and cancellation on historical SQL |
| [SINGLE_TABLE_QUERY_LIMITS](SINGLE_TABLE_QUERY_LIMITS.md) | [0042](../adr/0042-bounded-single-table-query-snapshots.md) | BEGIN snapshot / scan / projection budgets for single-table SQL |
| [RETAINED_HISTORY_LIMITS](RETAINED_HISTORY_LIMITS.md) | [0043](../adr/0043-retained-history-admission.md) | Per-domain retained-history key/version/accounted-byte caps |
| [HISTORY_STAGING_LIMITS](HISTORY_STAGING_LIMITS.md) | [0044](../adr/0044-historical-transaction-staging-limits.md) | Pending-key and combined pending+savepoint byte caps per transaction |
| [ROW_STORE_LIMITS](ROW_STORE_LIMITS.md) | [0045](../adr/0045-backing-row-map-admission.md) | Live and WAL-delta row-map budgets on the backing RowStore |
| [INDEX_RECOVERY_LIMITS](INDEX_RECOVERY_LIMITS.md) | [0046](../adr/0046-bounded-index-recovery-entries.md) | Retained checkpoint key/pointer caps for derived-index rebuild |
| [CHECKPOINT_BUILD_LIMITS](CHECKPOINT_BUILD_LIMITS.md) | [0047](../adr/0047-checkpoint-build-admission.md) | Checkpoint pointer-vector + descriptor working-state reservation |
| [METADATA_LIMITS](METADATA_LIMITS.md) | [0048](../adr/0048-bounded-archive-metadata.md) | Fixed-record read and archive-manifest JSON size caps |
| [SQL_OUTPUT_LIMITS](SQL_OUTPUT_LIMITS.md) | [0049](../adr/0049-bounded-sql-cli-output.md) | Per-line SQL CLI output admission in every mode |
| [HISTORY_TRANSACTION_ADMISSION](HISTORY_TRANSACTION_ADMISSION.md) | [0050](../adr/0050-historical-transaction-admission.md) | Per-domain active-transaction cap for SQL + fallible native BEGIN |
| [LOCAL_SQL_CLI](LOCAL_SQL_CLI.md) | [0040](../adr/0040-native-local-sql-cli.md), [0053](../adr/0053-private-root-local-access.md), [0057](../adr/0057-private-root-effective-uid.md) | The canonical `bradian-db` command and private-root enforcement |
| [DIRECTORY_LISTING_LIMITS](DIRECTORY_LISTING_LIMITS.md) | [0051](../adr/0051-bounded-directory-listings.md) | Entries and name-bytes in `RowStore::list_snapshots` and `legacy_migration::source_table_ids` |
| [CATALOG_LIMITS](CATALOG_LIMITS.md) | [0054](../adr/0054-decoded-catalog-admission.md) | Databases, schemas, tables, revisions, per-event bytes inside `PersistentCatalog` |
| [DOMAIN_LIMITS](DOMAIN_LIMITS.md) | [0056](../adr/0056-decoded-domain-admission.md) | Registered tables inside one `MultiTableStore` shared-WAL domain |
| [MVCC_STORE_LIMITS](MVCC_STORE_LIMITS.md) | [0058](../adr/0058-mvcc-store-admission.md) | Distinct keys and total version chain length in the opt-in G20 `MvccStore` |

## Security and fault documentation

These are not admission policies, but they describe runtime
invariants operators must know:

- [BRADIAN_THREAT_MODEL](../security/BRADIAN_THREAT_MODEL.md) — Gate 0 baseline, explicit blockers
- [BOOTSTRAP_API_AUTH](../security/BOOTSTRAP_API_AUTH.md) — loopback-only development auth
- [ADR 0052](../adr/0052-deterministic-fault-recovery.md) — deterministic fault recovery matrix

## What this index does not include

- External SQL reference compliance — the open G1-REFERENCE acceptance bar.
- Cross-platform installer, lock lifecycle and recovery runs on
  Windows / macOS.
- Global process-wide RSS or cgroup integration — an open §13
  owner-decision item in the
  [comprehensive implementation plan](../implementation/BRADIAN_COMPREHENSIVE_IMPLEMENTATION_PLAN.md).
- Distributed / cluster / federation / Studio / driver operations —
  blocked on §13 decisions.
