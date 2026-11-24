//! ADR 0313 — Federation (site autonomy).
//!
//! A federation registry binds sites to **persisted
//! federation links** and records every conflict
//! resolution to a durable audit log. Multi-site RPC and
//! clock-ordering remain follow-ups, but the link state
//! and audit trail that a reviewer would query after a
//! conflict event are durably recoverable across restart.

use std::collections::HashMap;
use std::path::PathBuf;

use bradian_core::Value;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::consensus::{ConsensusGroup, LocalConsensusLog, LogIndex};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd,
         Serialize, Deserialize)]
pub struct SiteId(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrustLevel {
    ReadOnlyRemote,
    AuthenticatedPeer,
    SharedTenancy,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FederationLink {
    pub local:  SiteId,
    pub remote: SiteId,
    pub trust:  TrustLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictResolution { PreferLocal, PreferRemote, Abort }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictAuditEntry {
    pub local_site:  SiteId,
    pub remote_site: SiteId,
    pub policy:      ConflictResolution,
    pub local_json:  String,
    pub remote_json: String,
    pub outcome:     ConflictOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConflictOutcome {
    ResolvedLocal,
    ResolvedRemote,
    Aborted,
}

#[derive(Debug, Error)]
pub enum FederationError {
    #[error("abort policy triggered on conflicting values")]
    AbortedConflict,
    #[error("durable federation io: {0}")]
    Log(String),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("federation log io: {0}")]
    Io(#[from] std::io::Error),
}
pub type FederationResult<T> = Result<T, FederationError>;

impl From<crate::consensus::ConsensusError> for FederationError {
    fn from(e: crate::consensus::ConsensusError) -> Self {
        FederationError::Log(format!("{e}"))
    }
}

/// In-memory federation registry. No durability.
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

// ---------------------------------------------------------------------------

type ReplayedFederation = (HashMap<SiteId, Vec<FederationLink>>, Vec<ConflictAuditEntry>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum FederationRecord {
    LinkAdded    { link: FederationLink },
    LinkRemoved  { local: SiteId, remote: SiteId },
    Conflict     { entry: ConflictAuditEntry },
}

/// Durable federation registry. Link state and conflict
/// audit entries persist to ADR 0309's consensus log.
#[derive(Debug)]
pub struct DurableFederation {
    log:    std::sync::Arc<LocalConsensusLog>,
    links:  Mutex<HashMap<SiteId, Vec<FederationLink>>>,
    audits: Mutex<Vec<ConflictAuditEntry>>,
}

impl DurableFederation {
    pub fn open(root: impl Into<PathBuf>) -> FederationResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e|
            FederationError::Log(format!("mkdir: {e}")))?;
        let log = std::sync::Arc::new(
            LocalConsensusLog::open(root.join("federation.log"))?);
        let (links, audits) = Self::replay(&log)?;
        Ok(Self { log, links: Mutex::new(links), audits: Mutex::new(audits) })
    }

    fn replay(log: &LocalConsensusLog) -> FederationResult<ReplayedFederation> {
        let mut links:  HashMap<SiteId, Vec<FederationLink>> = HashMap::new();
        let mut audits: Vec<ConflictAuditEntry>              = Vec::new();
        for i in 1..=log.committed_index().0 {
            let entry = log.entry(LogIndex(i)).expect("in-range");
            let rec: FederationRecord = serde_json::from_slice(&entry.bytes)?;
            match rec {
                FederationRecord::LinkAdded { link } => {
                    links.entry(link.local.clone()).or_default().push(link);
                }
                FederationRecord::LinkRemoved { local, remote } => {
                    if let Some(v) = links.get_mut(&local) {
                        v.retain(|l| l.remote != remote);
                    }
                }
                FederationRecord::Conflict { entry } => audits.push(entry),
            }
        }
        Ok((links, audits))
    }

    fn append(&self, rec: &FederationRecord) -> FederationResult<()> {
        let bytes = serde_json::to_vec(rec)?;
        self.log.append(bytes)?;
        Ok(())
    }

    pub fn link(&self, link: FederationLink) -> FederationResult<()> {
        self.append(&FederationRecord::LinkAdded { link: link.clone() })?;
        self.links.lock().entry(link.local.clone()).or_default().push(link);
        Ok(())
    }

    pub fn unlink(&self, local: &SiteId, remote: &SiteId) -> FederationResult<()> {
        self.append(&FederationRecord::LinkRemoved {
            local: local.clone(), remote: remote.clone() })?;
        if let Some(v) = self.links.lock().get_mut(local) {
            v.retain(|l| &l.remote != remote);
        }
        Ok(())
    }

    pub fn peers_of(&self, local: &SiteId) -> Vec<FederationLink> {
        self.links.lock().get(local).cloned().unwrap_or_default()
    }

    /// Resolve a conflict and durably audit the decision.
    /// `local_site` / `remote_site` are recorded so an
    /// auditor can later reconstruct which pair of sites
    /// produced the conflict.
    pub fn resolve_and_audit(
        &self,
        local_site:  SiteId,
        remote_site: SiteId,
        policy:      ConflictResolution,
        local:       Value,
        remote:      Value,
    ) -> FederationResult<Value> {
        let local_json  = serde_json::to_string(&local)?;
        let remote_json = serde_json::to_string(&remote)?;
        let (outcome, result) = if local == remote {
            // No conflict; still audit the no-op for a
            // visible trail.
            (ConflictOutcome::ResolvedLocal, Ok(local.clone()))
        } else {
            match policy {
                ConflictResolution::PreferLocal =>
                    (ConflictOutcome::ResolvedLocal,  Ok(local.clone())),
                ConflictResolution::PreferRemote =>
                    (ConflictOutcome::ResolvedRemote, Ok(remote.clone())),
                ConflictResolution::Abort =>
                    (ConflictOutcome::Aborted, Err(FederationError::AbortedConflict)),
            }
        };
        let entry = ConflictAuditEntry {
            local_site, remote_site, policy,
            local_json, remote_json, outcome,
        };
        self.append(&FederationRecord::Conflict { entry: entry.clone() })?;
        self.audits.lock().push(entry);
        result
    }

    pub fn audits(&self) -> Vec<ConflictAuditEntry> {
        self.audits.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(s: &str) -> SiteId { SiteId(s.into()) }

    #[test]
    fn link_is_visible_via_peers_of() {
        let r = FederationRegistry::new();
        let link = FederationLink {
            local:  site("a"), remote: site("b"),
            trust:  TrustLevel::AuthenticatedPeer,
        };
        r.link(link.clone());
        assert_eq!(r.peers_of(&site("a")), vec![link]);
        assert!(r.peers_of(&site("b")).is_empty());
    }

    #[test]
    fn resolver_follows_configured_policy() {
        let r = FederationRegistry::new();
        let l = Value::I64(10); let rm = Value::I64(20);
        assert_eq!(r.resolve(ConflictResolution::PreferLocal,  l.clone(), rm.clone()).unwrap(), l);
        assert_eq!(r.resolve(ConflictResolution::PreferRemote, l.clone(), rm.clone()).unwrap(), rm);
        assert!(r.resolve(ConflictResolution::Abort, l.clone(), rm.clone()).is_err());
        assert_eq!(r.resolve(ConflictResolution::Abort, l.clone(), l.clone()).unwrap(), l);
    }

    #[test]
    fn durable_links_recover_across_restart() {
        let dir = tempfile::tempdir().unwrap();
        {
            let f = DurableFederation::open(dir.path()).unwrap();
            f.link(FederationLink {
                local: site("a"), remote: site("b"),
                trust: TrustLevel::AuthenticatedPeer }).unwrap();
            f.link(FederationLink {
                local: site("a"), remote: site("c"),
                trust: TrustLevel::SharedTenancy }).unwrap();
        }
        let f = DurableFederation::open(dir.path()).unwrap();
        let peers = f.peers_of(&site("a"));
        assert_eq!(peers.len(), 2);
    }

    #[test]
    fn unlink_is_persisted_and_replayed() {
        let dir = tempfile::tempdir().unwrap();
        {
            let f = DurableFederation::open(dir.path()).unwrap();
            f.link(FederationLink {
                local: site("a"), remote: site("b"),
                trust: TrustLevel::AuthenticatedPeer }).unwrap();
            f.link(FederationLink {
                local: site("a"), remote: site("c"),
                trust: TrustLevel::SharedTenancy }).unwrap();
            f.unlink(&site("a"), &site("b")).unwrap();
        }
        let f = DurableFederation::open(dir.path()).unwrap();
        let peers = f.peers_of(&site("a"));
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].remote, site("c"));
    }

    #[test]
    fn conflict_audit_records_every_resolution() {
        let dir = tempfile::tempdir().unwrap();
        {
            let f = DurableFederation::open(dir.path()).unwrap();
            // Local wins
            let v = f.resolve_and_audit(
                site("home"), site("peer"),
                ConflictResolution::PreferLocal,
                Value::I64(1), Value::I64(2)).unwrap();
            assert_eq!(v, Value::I64(1));
            // Remote wins
            let v = f.resolve_and_audit(
                site("home"), site("peer"),
                ConflictResolution::PreferRemote,
                Value::I64(1), Value::I64(2)).unwrap();
            assert_eq!(v, Value::I64(2));
            // Abort
            let err = f.resolve_and_audit(
                site("home"), site("peer"),
                ConflictResolution::Abort,
                Value::I64(1), Value::I64(2)).unwrap_err();
            assert!(matches!(err, FederationError::AbortedConflict));
        }
        let f = DurableFederation::open(dir.path()).unwrap();
        let a = f.audits();
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].outcome, ConflictOutcome::ResolvedLocal);
        assert_eq!(a[1].outcome, ConflictOutcome::ResolvedRemote);
        assert_eq!(a[2].outcome, ConflictOutcome::Aborted);
    }

    #[test]
    fn equal_values_do_not_trigger_abort_even_under_abort_policy() {
        let dir = tempfile::tempdir().unwrap();
        let f = DurableFederation::open(dir.path()).unwrap();
        let v = f.resolve_and_audit(
            site("x"), site("y"),
            ConflictResolution::Abort,
            Value::Str("same".into()), Value::Str("same".into()),
        ).unwrap();
        assert_eq!(v, Value::Str("same".into()));
    }
}
