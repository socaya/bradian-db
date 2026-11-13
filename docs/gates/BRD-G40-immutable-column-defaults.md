# BRD-G40 — Immutable typed column defaults

**Design:** [ADR 0035](../adr/0035-immutable-constant-column-defaults.md)

Historical SQL supports exclusive ADD COLUMN with a typed constant DEFAULT,
including NOT NULL columns. Old rows project the default stored in their birth
schema transition; INSERT omissions and explicit DEFAULT use the same metadata.
Explicit NULL remains distinct and must satisfy nullability. Original rows and
schema pins remain unchanged. Other SQL engines keep their earlier contracts.

```sql
ALTER TABLE bank.public.accounts ADD COLUMN status TEXT NOT NULL DEFAULT 'active';
INSERT INTO bank.public.accounts (id, balance) VALUES (3, 300);
INSERT INTO bank.public.accounts (id, balance, status) VALUES (4, 400, DEFAULT);
DESCRIBE TABLE bank.public.accounts AS OF 6 WITH DEFAULTS;
```

Use `execute_ddl` on a mutable HistorySqlEngine after enabling G35 epoch writes.
INSERT and DESCRIBE use the existing SQL session. The native
`add_column_with_default` method validates a typed Value through the same path.
WITH DEFAULTS adds has_default and column_default to historical DESCRIBE; without
it the original four-column result remains unchanged.

Defaults accept compatible boolean, numeric, text and NULL literals; declared
precision, scale, length and nullability are checked before any durable write.
Functions, expressions, DEFAULT VALUES and SET/DROP DEFAULT remain unsupported.
Existing-column defaults cannot be added, changed or removed through this API.

A distinct catalog command and version-2 history intent/activation prevent older
readers from silently ignoring default metadata. Empty default maps preserve exact
legacy schema serialization. The original G33/G39 planners remain strict. Recovery
retains all four G36 durable boundaries, exact catalog horizons, live-owner fencing,
retention validation and verified backup/restore.

Eleven new tests exercise SQL defaults, invalid input/rollback, maximum-precision
decimals, mixed epochs, historical metadata, retention, unchanged physical rows,
backup/restore, legacy serialization, immutable planner rules, all four recovery
boundaries, sixteen torn frames and intent/activation downgrade refusal. See the
[executed report](../implementation/BRADIAN_G40_VERIFICATION_2026_11_13.md).

Concurrent online DDL, existing-data constraint tightening/backfill, expressions,
DROP/RENAME/type changes and broader SQL remain open in the gap register.
