# ADR 0313 — Federation (site autonomy): skeleton

## Status

Accepted as a skeleton. Sixth slice of P2.

## Context

Multi-site deployments need explicit site autonomy: each
site owns its catalog + data, and cross-site reads
compose via a federation link with explicit
conflict / partition semantics.

## Decision

New module `bradian_distributed::federation` with:

```rust
pub struct SiteId(pub String);
pub struct FederationLink { pub local: SiteId, pub remote: SiteId, pub trust: TrustLevel }
pub enum TrustLevel { ReadOnlyRemote, AuthenticatedPeer, SharedTenancy }
pub enum ConflictResolution { PreferLocal, PreferRemote, Abort }

pub struct FederationRegistry { ... }
impl FederationRegistry {
    pub fn link(&self, link: FederationLink) -> FederationResult<()>;
    pub fn peers_of(&self, local: &SiteId) -> Vec<FederationLink>;
    pub fn resolve(&self, policy: ConflictResolution, local: Value, remote: Value) -> Value;
}
```

Honest scope: in-memory registry + pure resolver. No RPC,
no clock ordering, no cross-site WAL shipping.

## Consequences

- 2 unit tests: adding a link exposes it via `peers_of`;
  resolver follows the policy.
