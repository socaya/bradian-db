//! ADR 0313 — Federation (site autonomy) **skeleton**.
//!
//! In-memory registry + pure conflict resolver. No RPC,
//! no clock ordering, no cross-site WAL shipping.

use std::collections::HashMap;

use bradian_core::Value;
use parking_lot::Mutex;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct SiteId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustLevel {
    ReadOnlyRemote,
    AuthenticatedPeer,
    SharedTenancy,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FederationLink {
    pub local:  SiteId,
    pub remote: SiteId,
    pub trust:  TrustLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictResolution { PreferLocal, PreferRemote, Abort }

#[derive(Debug, Error)]
pub enum FederationError {
    #[error("abort policy triggered on conflicting values")]
    AbortedConflict,
}

pub type FederationResult<T> = Result<T, FederationError>;

#[derive(Debug, Default)]
pub struct FederationRegistry {
    links: Mutex<HashMap<SiteId, Vec<FederationLink>>>,
}

impl FederationRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn link(&self, link: FederationLink) {
        self.links.lock().entry(link.local.clone()).or_default().push(link);
    }

    pub fn peers_of(&self, local: &SiteId) -> Vec<FederationLink> {
        self.links.lock().get(local).cloned().unwrap_or_default()
    }

    pub fn resolve(
        &self, policy: ConflictResolution,
        local:  Value, remote: Value,
    ) -> FederationResult<Value> {
        if local == remote { return Ok(local); }
        match policy {
            ConflictResolution::PreferLocal  => Ok(local),
            ConflictResolution::PreferRemote => Ok(remote),
            ConflictResolution::Abort        => Err(FederationError::AbortedConflict),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_is_visible_via_peers_of() {
        let r = FederationRegistry::new();
        let link = FederationLink {
            local:  SiteId("a".into()),
            remote: SiteId("b".into()),
            trust:  TrustLevel::AuthenticatedPeer,
        };
        r.link(link.clone());
        assert_eq!(r.peers_of(&SiteId("a".into())), vec![link]);
        assert!(r.peers_of(&SiteId("b".into())).is_empty());
    }

    #[test]
    fn resolver_follows_configured_policy() {
        let r = FederationRegistry::new();
        let l = Value::I64(10); let rm = Value::I64(20);
        assert_eq!(r.resolve(ConflictResolution::PreferLocal,  l.clone(), rm.clone()).unwrap(), l);
        assert_eq!(r.resolve(ConflictResolution::PreferRemote, l.clone(), rm.clone()).unwrap(), rm);
        assert!(r.resolve(ConflictResolution::Abort, l.clone(), rm.clone()).is_err());
        // Equal values are not a conflict.
        assert_eq!(r.resolve(ConflictResolution::Abort, l.clone(), l.clone()).unwrap(), l);
    }
}
