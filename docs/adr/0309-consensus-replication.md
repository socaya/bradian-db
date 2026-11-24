# ADR 0309 — Consensus-backed replication log

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
durable append-only consensus log** (the single-replica-
leader case of Raft).

## Context

The skeleton exposed a `ConsensusGroup` trait with an
in-memory `InMemoryConsensusGroup` that evaporated at
process exit. A real distributed Bradian needs a log that
persists committed entries across crash + restart so a
leader restoring state can trust the entries it previously
acknowledged.

## Decision

Two implementations of `ConsensusGroup`:

```rust
pub struct InMemoryConsensusGroup;   // unchanged — tests + ephemeral coord
pub struct LocalConsensusLog;        // durable append-only on-disk log
pub struct LocalConsensusRegistry;   // per-shard log registry under one root
```

`LocalConsensusLog` on-disk format:

- File header: 8-byte magic `BRCL\x00\x00\x00\x01`.
- Records concatenated after the header:
  `CREC` (4) | term (8 LE) | index (8 LE) | len (4 LE) |
  payload (len bytes) | crc32 (4 LE) over
  `term||index||len||payload`.

Guarantees:

- `append` writes one record + `sync_data()` before
  returning the committed index.
- `open` replays the file. Any trailing torn record
  (missing magic / truncated payload / CRC mismatch) is
  detected and the file is **truncated** at the
  last-known-good offset, so the log always presents a
  consistent prefix.
- Index must be monotonically increasing; a hole in the
  recovered sequence is treated as corruption and
  surfaced.
- A missing file-magic header is a hard error (not
  truncatable).
- `LocalConsensusRegistry::open_group(name)` caches live
  handles so a coordinator handing out logs by shard name
  gets the same `Arc<LocalConsensusLog>` on every call.

## Scope

**In scope:**
- Durable, length-prefixed, CRC-stamped record format.
- `fsync_data` per append.
- Torn-tail trimming on reopen.
- Index monotonicity + file magic validation.
- 8 unit tests: 2 preserved from the skeleton (in-memory
  group) + 6 new ones covering reopen round-trip (32
  entries), post-reopen sequence continuation, torn
  trailing-bytes trimming, single-byte CRC mismatch
  trimming, missing file-magic rejection, and the
  registry's cached-handle behaviour.

**Out of scope** (dedicated follow-ups):
- Multi-replica replication — vote RPCs, AppendEntries,
  log matching, leader election, membership change.
- Log compaction / snapshots.
- Encryption at rest.
- A Raft state machine on top of this log is where the
  replication layer attaches.

## Consequences

- An acknowledged `append` survives crash + restart with
  a well-defined trimming rule.
- The 2026-11-24 "SKELETON" status on 0309 is retired.
- `bradian-distributed` adds `crc32fast` as a workspace
  dep (already present in `bradian-relational` and
  `bradian-sources` for the same reason).
