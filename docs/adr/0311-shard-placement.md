# ADR 0311 — Shard placement + fencing

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
durable placement manifest** that round-trips across
restart.

## Context

The skeleton exposed a `PlacementMap` with per-shard
monotone fence tokens but stored everything in a
`HashMap<ShardId, Placement>` that evaporated on process
restart. A real distributed deployment needs ownership
decisions to survive restart; otherwise a crashed primary
could reclaim a shard whose fence had already advanced.

## Decision

Single type `PlacementMap` with two constructors:

- `PlacementMap::new()` — in-memory, process-local.
- `PlacementMap::open(root)` — durable, backed by a JSON
  manifest at `root/placements.json`.

The on-disk manifest is a versioned JSON document:

```json
{ "version": 1,
  "placements": [
    { "shard": {...}, "primary": {...}, "fence": {...} },
    ...
  ] }
```

Every `assign` on the durable variant:

1. Takes the in-memory mutex.
2. Advances the per-shard fence.
3. Serialises the whole map into `placements.tmp` and
   `sync_data()` on the file.
4. `rename`s `placements.tmp` over `placements.json`.
5. On Unix, best-effort `sync_data()` on the directory
   so the rename is durably observable.

`open` replays the manifest on reconstruction:

- A missing root is created; a missing manifest produces
  an empty map.
- Corrupt JSON or an unsupported `version` field fails
  closed with typed errors.
- Duplicate shards in the on-disk array are rejected as
  corruption.

## Scope

**In scope:**
- Durable `assign` via tmp-file + atomic rename + `fsync`.
- Replay of all persisted placements on `open`.
- Fence monotonicity preserved **across restart**: a
  reopened map produces `fence = persisted_fence + 1` on
  its next assign of the same shard.
- Typed `PlacementError` surface for IO / serde /
  version / corrupt errors.
- 8 unit tests: original 2 skeleton tests + 6 new ones
  covering reopen round-trip, post-restart fence
  monotonicity, missing-root creation, corrupt JSON
  rejection, unsupported-version rejection, and
  atomic-rename-against-concurrent-reads invariant.

**Out of scope** (dedicated follow-ups):
- Consensus replication of the map across replicas
  (ADR 0309 is where the log transport will live).
- Live reconfig protocol (which replica adopts a shard
  after a primary failure).
- Shared concurrent writers across processes — a single
  `assign` writer per `open` is the current contract.

## Consequences

- An operator can restart a Bradian node and expect its
  shard ownerships + fences to come back unchanged.
- `consensus::{ReplicaId, Term, LogIndex}` and
  `shard_placement::{ShardId, FencingToken, Placement}`
  are now `Serialize + Deserialize` (needed so the
  manifest can round-trip).
- No new workspace deps; `serde_json`, `parking_lot`,
  `thiserror` were already pulled in.
- The 2026-11-24 "SKELETON" status on 0311 is retired.
