# ADR 0031 — Recoverable additive historical DDL

**Status:** Accepted for exclusive, single-process nullable-column additions.

An explicit ALTER decision is journaled in the G22 history WAL before the G11
catalog changes. An intent binds an immutable previous history schema epoch,
complete proposed table and exact catalog WAL horizon. Its key is its own history
commit LSN. The catalog then commits the existing proposal and activation events
at horizon+1 and horizon+2. A final history activation record references the intent
and horizon+2; its history commit position becomes the new immutable schema epoch.

The owner holds both stores' exclusive locks. DDL requires mutable engine access
and no pinned row transactions, G35 encoding enabled, no pending catalog proposals
and no unretired legacy G28–G31 metadata. The live operation excludes row sessions. These
legacy intents have a different immutable-horizon protocol and require explicit
[ADR 0032 retirement](0032-legacy-ddl-intent-retirement.md) rather than silent consumption. ALTER of a clean root uses the protocol above directly.

The history intent is a durable roll-forward decision. On reopen, validate the
entire history and the proposed transition before modifying the catalog. Accept
only the exact three interrupted catalog states: original horizon/table and no
proposal; horizon+1 with the matching pending proposal; or horizon+2 with the
matching activated table. Complete missing catalog steps and the final history
activation before serving SQL. Independent divergence is an error. A failed live
operation fences the engine until reopen; no partially transitioned owner serves
rows. No record is silently rolled back after an intent is synced.

Original schema pins and row bytes are immutable. All schema events are immutable
singleton history keys. BRRW rows decode under the original pin; BRVE rows decode
under their exact epoch. Every retained row version must have used the schema
active at its commit. Reads project validated older rows into a requested later
additive schema by appending NULLs. AS OF binds columns to the schema at that LSN.

Repeated additions create a verifiable chain of catalog horizons and history
epochs. Checkpoints and retention preserve singleton metadata. Backup may include
only fully completed transitions and must verify exact catalog/history horizons
before publication. Older readers reject the new metadata namespaces.

The public DDL profile is ALTER TABLE db.schema.table ADD [COLUMN] name type [NULL].
NOT NULL, defaults, removals, renames and type/constraint changes remain refused.
This is not concurrent online DDL, distributed DDL, full SQL or power-loss
qualification. Journal entries remain subject to the existing bounded inline limit.
