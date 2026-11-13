# BRD-G36 — Recoverable nullable-column ALTER and historical schemas

**Design:** [ADR 0031](../adr/0031-recoverable-additive-historical-ddl.md)

**Implemented:** exclusive single-process DDL through `HistorySqlEngine::execute_ddl`
and `add_nullable_column`, with immutable schema epochs and coordinated recovery.

```rust
let mut db = HistorySqlEngine::open(existing_root)?;
// Required once for a legacy G23 root:
if !db.epoch_row_writes_enabled()? {
    db.enable_epoch_row_writes()?;
}
let receipt = db.execute_ddl(
    "ALTER TABLE bank.public.accounts ADD COLUMN note TEXT NULL"
)?;
```

Repeated additions preserve table IDs, original columns and constraints. Untouched
BRRW rows decode under their original schema, BRVE rows under the exact registered
epoch. Current reads append NULLs for added columns. INSERT/UPDATE writes use the
new epoch. AS OF point reads, scans, SELECT * and DESCRIBE bind the schema active
at the requested history LSN, so newly added columns cannot leak into older queries.

## Publication and recovery

| Durable step | State | Recovery |
|---|---|---|
| History intent | Exact previous epoch, proposed definition and catalog horizon | Complete the missing catalog proposal and activation |
| Catalog proposal | G34 proposal at catalog horizon+1 | Verify the exact receipt and activate it |
| Catalog activation | Table changed at catalog horizon+2 | Commit the missing history activation |
| History activation | Commit at intent LSN+1 becomes the new schema epoch | Verify both stores; reopening adds no records |

The history intent is a roll-forward decision. Validate all retained payloads and
metadata before modifying the catalog during recovery. Extra writes, wrong epochs,
unrelated catalog changes, rewritten/deleted records and forged chronology fail.
A failed live transition fences row sessions, checkpoints, backups and further DDL
until reopen. A standalone typed adapter refuses unfinished transitions.

DDL requires mutable engine access, no active snapshots, G35 encoding, and no
pending G33 catalog proposals. Unretired G28–G31 work is refused; see
[G37](BRD-G37-legacy-ddl-retirement.md) for its explicit upgrade path. The schema
payload is bounded by the existing inline journal budget with catalog wrapper space
reserved before the first write. Older readers reject the new namespace.

## Evidence and limits

Eight SQL integration tests cover the capability prerequisite, untouched-row NULL
projection, repeated revisions, mixed-epoch DML, historical column binding,
unsupported syntax/no-write errors, backup/restore, retention, horizon divergence
and stale-row-epoch corruption. Ten unit recovery/integrity tests cover all four
commit boundaries, live-owner fencing, sixteen torn-frame cases, idempotent reopen,
standalone refusal, forged intents/commits/retirement receipts, immutable record
protection and complete-WAL CRC refusal. Tests use real local WALs; interruption
hooks inject errors after durable steps without exposing a production fault flag.

This profile does not support concurrent online DDL, NOT NULL/default backfills,
DROP/RENAME/type changes, cross-process SQL transactions or distributed DDL. It is
not physical device power-loss qualification. See the
[executed verification report](../implementation/BRADIAN_VERIFICATION_2026_11_13.md).
