# BRD-G35 — Live schema epoch row encoding

**Design:** [ADR 0030](../adr/0030-live-schema-epoch-row-encoding.md)

**Implementation:** `durable_typed.rs`, `history_sql.rs`, existing G32 `epoch_row.rs`.

## Delivered behavior

The G23 historical SQL engine can explicitly opt in to G32 BRVE row writes. This
commits one immutable capability marker in the existing history WAL. The marker
and row changes use the same G22 transaction mechanism; no catalog change or
second WAL is involved. Enabling advances history by one commit and does not
rewrite existing rows, original schemas, catalog metadata or old history LSNs.
The call is a one-time transition; duplicate calls fail without another commit.
Status is visible on the same engine handle immediately after success.

```rust
let engine = HistorySqlEngine::open(existing_history_root)?;
let format_lsn = engine.enable_epoch_row_writes()?;
assert!(engine.epoch_row_writes_enabled()?);
```

Transactions begun after `format_lsn` encode INSERT/UPDATE rows in BRVE, binding
each row to the exact table ID and original schema-registration WAL position.
Transactions begun earlier retain BRRW writes even if they commit later. Both
formats coexist. SQL point reads, scans, updates, deletes and historical AS OF
queries decode the matching format and verify the physical primary key. The
registry is reconstructed from verified immutable schema pins on reopen.

G24 backup/restore and checkpoints preserve the marker and both formats. G28–G31
prepared proposals and coordination/readiness fences remain inactive and valid.
Retention keeps baseline schema/format markers; queries below the floor fail.

## Validation and capacity

Reopen validates every retained row version, including overwritten rows hidden
by a tombstone. It rejects unsupported BRVE versions, checksum/length failures,
unknown/zero epochs, wrong table identities, mismatched physical PKs, missing or
late format markers and rewritten/deleted/unknown encoding decisions. A BRVE row
must follow the format activation commit; all original pins must predate it.
Readers older than G35 reject the new capability namespace rather than silently
opening an upgraded store. There is no supported downgrade after enabling.

BRVE adds 29 bytes to the existing BRRW payload, within the same 3545-byte G22
value limit. Large legacy rows remain readable; rewriting them as BRVE may
require a smaller value. SQL errors abort the staged transaction without
publishing its earlier writes.

## Executed coverage

Twelve new integration tests cover:

- Explicit opt-in, unchanged original row bytes/catalog and same-handle status.
- Live mixed-format multi-table CRUD, point/scan AS OF and restart/checkpoint.
- Transaction snapshots spanning the transition and racing opt-in calls.
- G24 restore, retained history and G28–G31 metadata coexistence.
- Exact inline boundary, oversize-row refusal and transaction rollback.
- Retention after multiple versioned writes.
- Eight malformed-envelope cases hidden behind tombstones.
- Three invalid marker/row orderings and four marker corruption cases.
- Five partial activation-frame truncations, repair and successful retry.
- Process exit without destructors followed by successful SQL recovery.

See the [13 November verification report](../implementation/BRADIAN_VERIFICATION_2026_11_13.md)
for the full workspace result and environment.

## Remaining boundary

This implements live epoch-tagged row storage under immutable table schemas.
It does not activate a G34 catalog revision in a historical root, provide SQL
ALTER TABLE, map records between active schema revisions, or coordinate two WALs.
Existing exact catalog/schema pins and G30 horizon fences remain mandatory.
Cross-platform power-loss, device failure and production qualification remain open.
Automatic CI remains manual-only.

## Follow-up

[G36](BRD-G36-recoverable-historical-alter.md) adds exclusive additive schema activation
and historical schema-aware SQL. [G37](BRD-G37-legacy-ddl-retirement.md) provides the
explicit upgrade path for inactive legacy intents. The immutable-only boundary
above describes G35 at its introduction.
