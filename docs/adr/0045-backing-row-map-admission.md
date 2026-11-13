# ADR 0045 — Backing RowStore map admission (G1-MEMORY)

**Status:** Accepted for per-RowStore live-row and WAL-delta maps. Gate 1 remains active.

Derived-history and transaction staging policies do not protect RowStore's earlier
checkpoint materialization and WAL replay. Add independent RowStoreLimits and exact
RowStoreUsage for the authoritative live-row map and committed delta overlay.
Defaults are 2000000 live rows, 256 MiB live-row accounted bytes, 2000000 delta keys
and 256 MiB delta accounted bytes. Byte estimates include key/value payload and
native vector/option entry headers, not allocator/container slack or process RSS.
All keys count, including metadata and absent-delete delta tombstones.

Default open uses these policies across existing RowStore consumers. Native
open_with_limits allows explicit policies; they are not persisted. Existing higher
SQL/history APIs retain these backing defaults independently of their query/history/
staging policies. This slice does not add SQL/CLI overrides for backing-map policy.

Checkpoint loading checks each borrowed decoded row before copying it into the map.
WAL replay decodes/validates a bounded frame, then admits its complete final map
changes before applying it. Every checkpoint and replayed commit prefix must fit;
a smaller final state alone does not permit allocating an over-policy earlier state.
Quota refusal never trims the WAL, publishes partial mutations or auto-checkpoints.
Derived-index repair and incomplete-tail trimming follow successful map admission.
Normal tail repair and corruption checks retain their existing success contracts.

Live mutation admission runs under the same mutex as WAL publication. Coalesce
repeated batch keys into borrowed final mutations, subtract all affected old charges,
then add final live/delta charges. Validate before encoding or appending WAL. Remove
affected old map entries before copying final entries, so original operation order
cannot create a larger temporary map than admitted final state. WAL bytes remain
unchanged; replay uses the same coalescing and accounting. Resource refusal leaves
the healthy owner usable and does not invoke failed-I/O poisoning.

Refund delta charges only after checkpoint publication, WAL truncation and sync
succeed. Deletes free live rows but retain delta tombstones until checkpoint. Explicit
backup verification inherits source policy. restore_backup_with_limits accepts the
destination policy and verifies the staged copy before publication. Failed restore
keeps its existing incomplete-stage contract. Backup/source comparison visits borrowed
rows instead of cloning another full recovered map.

These are per-map accounted-state ceilings, not a shared memory reservation. Bounded
frame/page decode workspace, metadata/directory enumeration, cache/index/checkpoint/
backup audit buffers, allocator slack, higher-level catalog/history structures,
aggregate transactions/pins and alternate engines remain separate. Native callers
can raise limits explicitly; opening under lower limits may first require a higher-
policy open and explicit checkpoint. No dependency, WAL/page/archive format, default
isolation, automatic retention or production memory/fault certification is introduced.
