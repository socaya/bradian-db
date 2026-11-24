# ADR 0301 — Canonical change-log schema + idempotent replay

## Status

Accepted. First slice of P1-4 (ingest + source adapters).

## Context

`bradian_ingest::envelope::Change` already carries a stable
`change_id` and `source_version`, and `Partition::upsert_row`
fences by `source_version` so writing an older version is a
no-op. That gives **per-row** replay safety but not
**per-change** idempotency: a re-applied change can still
quarantine twice, bump the ingest report twice and run the
delete side-effect twice if the versions are equal.

P1-4 slice 1 acceptance: "Idempotent ingest key + replay."

## Decision

Add an explicit, in-memory **change-ID index** to the
ingester as an opt-in behaviour:

```rust
pub struct Ingester { ... seen_change_ids: Option<Mutex<HashSet<String>>>, ... }
impl Ingester {
    pub fn with_idempotent_dedup(mut self) -> Self { ... }
}
pub enum Outcome { Accepted, Deleted, Stale, Quarantined, DuplicateChangeId }
```

Semantics:

- `with_idempotent_dedup()` enables the index; by default
  the ingester behaves as before (no dedup → backwards
  compatible with existing callers).
- When enabled, `apply` and `apply_batch` check the
  change-ID index BEFORE any partition / quarantine side
  effect. A hit returns `Outcome::DuplicateChangeId` and
  increments a new `IngestReport::duplicates` counter;
  the partition and quarantine are untouched.
- The index is in-memory and process-local. A durable
  index (so idempotency survives restart) is explicitly
  out of scope for this slice; the ingester docstring
  calls that out.

## Consequences

- Replaying a batch of changes through the same
  dedup-enabled ingester produces no second-round
  side effects.
- `IngestReport` grows one field. Existing consumers
  reading only `accepted / dropped_stale / deletes /
  quarantined` continue to compile.
- 3 new unit tests in `ingester.rs`: single replay of one
  change; replay of a 10-change batch (all deduped);
  mixed batch with new and already-seen change IDs.
- No change to the on-disk format or the HTTP envelope.
