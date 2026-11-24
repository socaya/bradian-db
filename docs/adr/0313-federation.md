# ADR 0313 — Federation (site autonomy)

## Status

Accepted. Replaces the 2026-11-24 skeleton with a **real
durable federation registry** that persists link state and
audits every conflict resolution.

## Context

The skeleton exposed `FederationRegistry` with an
in-memory link map and a `resolve()` that returned a
value based on policy. A conflict handled in-memory left
no trail — a reviewer who later asked "did site A ever
discard a value from site B?" had nothing to query.

## Decision

New `DurableFederation`:

```rust
pub struct DurableFederation { ... }
impl DurableFederation {
    pub fn open(root: impl Into<PathBuf>) -> FederationResult<Self>;
    pub fn link  (&self, link: FederationLink)       -> FederationResult<()>;
    pub fn unlink(&self, local: &SiteId, remote: &SiteId) -> FederationResult<()>;
    pub fn peers_of(&self, local: &SiteId) -> Vec<FederationLink>;
    pub fn resolve_and_audit(&self,
        local_site:  SiteId,  remote_site: SiteId,
        policy:      ConflictResolution,
        local:       Value,   remote:      Value,
    ) -> FederationResult<Value>;
    pub fn audits(&self) -> Vec<ConflictAuditEntry>;
}
```

Backed by ADR 0309's `LocalConsensusLog` at
`{root}/federation.log`. Each record is a JSON
`FederationRecord`:

- `LinkAdded { link }` / `LinkRemoved { local, remote }`
  — the live link graph.
- `Conflict { entry }` — audit entry capturing the two
  site IDs, the policy, the two JSON-serialised values,
  and the outcome.

Semantics:

- `link` / `unlink` durably append before returning.
- `resolve_and_audit` serialises both values to JSON,
  records the policy outcome, and appends an audit
  entry whether the resolution is Local-wins, Remote-wins,
  or Abort (Abort returns `FederationError::AbortedConflict`
  to the caller but STILL audits).
- Equal values do not trigger Abort even under the Abort
  policy — a no-op conflict is still audited with
  `ResolvedLocal` outcome.
- `open(root)` replays the log; links are reconstructed
  in order, so an `unlink` after `link` for the same
  pair leaves the pair absent as expected.

## Scope

**In scope:**
- Durable link graph + unlink.
- Durable per-conflict audit log replayable on open.
- 6 unit tests: 2 preserved from the skeleton (link
  visibility + resolver policy) plus 4 new ones:
  durable links recover across restart,
  unlink persists and replays, conflict audits record
  every resolution + outcome (Local / Remote / Abort),
  equal values do not abort even under Abort policy.

**Out of scope** (dedicated follow-ups):
- Real cross-site RPC (replication of the link graph).
- Clock ordering / hybrid logical clocks.
- Site-provenance tagging on rows (upstream of federation
  — a `bradian-core::Provenance` extension).

## Consequences

- An operator can run a crash-safe federation registry
  and later export the full conflict audit trail.
- `SiteId` loses `Copy` (it wraps a `String`) and gains
  `Ord + PartialOrd` for sorted-by-site queries.
- The 2026-11-24 "SKELETON" status on 0313 is retired.
