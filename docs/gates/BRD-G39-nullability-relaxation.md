# BRD-G39 — Recoverable NOT NULL relaxation and implicit INSERT NULL

**Design:** [ADR 0034](../adr/0034-nullability-relaxation-and-insert-null.md)

## Delivered behavior

Historical SQL supports exclusive `ALTER TABLE ... ALTER [COLUMN] ... DROP NOT NULL`.
The new `HistorySqlEngine::drop_not_null` API and SQL parser use the existing G36
intent, catalog proposal/activation and history activation protocol. Earlier schemas
and physical rows remain unchanged. Repeated nullable additions and constraint
relaxations form one verified schema epoch chain.

```sql
ALTER TABLE bank.public.accounts ALTER COLUMN balance DROP NOT NULL;
INSERT INTO bank.public.accounts (id) VALUES (3);
```

Use `execute_ddl` on a mutable engine for ALTER, after explicit G35 epoch encoding
has been enabled. INSERT runs inside the existing BEGIN/COMMIT SQL session. Omitted
nullable columns receive NULL, including newly added columns; omitted NOT NULL or
primary-key columns still fail and abort staged work. Other SQL engines retain their
existing INSERT contracts.

Older rows project to a relaxed schema without changing their values. A subsequent
NULL write uses the new epoch. Historical point reads, scans and DESCRIBE still bind
to the earlier NOT NULL definition and corresponding earlier data. Source payloads
are always decoded/validated before projection, and commit-time epoch validation
remains mandatory.

## Catalog compatibility and recovery

G11 adds `ProposeNullableRevision` for the broader, explicitly checked projection.
G33's original `propose_table_revision` remains nullable-additions-only. A primary
key cannot become nullable. Reordering, renames, type changes, dropped columns,
nullability narrowing and no-op revisions remain errors before durable writes.

The coordinator preserves the old proposal command for strict additions and uses
the new command only for relaxation. Both bind the exact proposed table and catalog
LSN. G36's four durable boundaries, failed-owner fences, schema history and backup
verification remain in force. Old readers reject a relaxation intent/new catalog
command; there is no silent downgrade or file rewrite.

## Executed evidence

Twelve new tests cover seven end-to-end SQL scenarios, two planner cases, preservation
of the original catalog API contract, all four interrupted relaxation boundaries and
sixteen torn relaxation-frame cases. Coverage includes NULL writes, omitted values,
required-field rollback, PK/no-op/narrowing refusal, unchanged original rows/pins,
mixed schema epochs, historical nullability inspection, multi-table writes,
backup/restore, retention, and upgrade from explicitly retired legacy intents.

See the [G39 verification report](../implementation/BRADIAN_G39_VERIFICATION_2026_11_13.md).
Non-NULL defaults, SET NOT NULL validation/backfill, DEFAULT VALUES syntax, dropped
columns, renames, type changes and concurrent online DDL remain separate work.
