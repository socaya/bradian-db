//! Residency and privacy-first fragment placement (DST-004).
//!
//! Each site publishes a `ResidencyPolicy` declaring what classification of data may leave
//! it. The coordinator evaluates the policy before dispatching a fragment and refuses to
//! send any fragment that would violate the declared residency.

use ahash::AHashSet;
use dap_core::Uid;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DataClassification {
    /// Raw patient-level data (personally identifying).
    Raw,
    /// Pseudonymised (one-way hashed) identifiers.
    Pseudonymised,
    /// Aggregate counts only.
    Aggregate,
    /// No data may leave.
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResidencyPolicy {
    /// The highest classification the site allows to leave.
    pub allowed_egress: DataClassification,
    /// Whitelist of OU UIDs that may be queried remotely. Empty = all under this site.
    #[serde(default)]
    pub allowed_ou_scope: AHashSet<Uid>,
    /// Description surfaced in audit output.
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResidencyViolation {
    /// Fragment asks for a higher-classification response than the site allows.
    EgressClassificationExceeded {
        requested: DataClassification,
        allowed: DataClassification,
    },
    /// Fragment names an OU outside the site's allowed scope.
    OutsideOuScope { offending: Vec<Uid> },
}

impl ResidencyPolicy {
    pub fn new(allowed_egress: DataClassification) -> Self {
        Self {
            allowed_egress,
            allowed_ou_scope: AHashSet::default(),
            note: String::new(),
        }
    }

    pub fn permit_ou(mut self, uid: Uid) -> Self {
        self.allowed_ou_scope.insert(uid);
        self
    }

    pub fn check(
        &self,
        request_classification: DataClassification,
        request_ou_scope: &[Uid],
    ) -> Result<(), ResidencyViolation> {
        if !classification_allows(self.allowed_egress, request_classification) {
            return Err(ResidencyViolation::EgressClassificationExceeded {
                requested: request_classification,
                allowed: self.allowed_egress,
            });
        }
        if !self.allowed_ou_scope.is_empty() {
            let offenders: Vec<Uid> = request_ou_scope
                .iter()
                .filter(|u| !self.allowed_ou_scope.contains(u))
                .cloned()
                .collect();
            if !offenders.is_empty() {
                return Err(ResidencyViolation::OutsideOuScope {
                    offending: offenders,
                });
            }
        }
        Ok(())
    }
}

fn classification_allows(
    allowed: DataClassification,
    requested: DataClassification,
) -> bool {
    use DataClassification::*;
    match (allowed, requested) {
        (None, _) => false,
        (_, None) => true,
        (Raw, _) => true,
        (Pseudonymised, Raw) => false,
        (Pseudonymised, _) => true,
        (Aggregate, Aggregate) => true,
        (Aggregate, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    #[test]
    fn none_refuses_everything() {
        let p = ResidencyPolicy::new(DataClassification::None);
        assert!(p.check(DataClassification::Aggregate, &[]).is_err());
    }

    #[test]
    fn aggregate_allows_aggregate() {
        let p = ResidencyPolicy::new(DataClassification::Aggregate);
        assert!(p.check(DataClassification::Aggregate, &[]).is_ok());
        assert!(p.check(DataClassification::Raw, &[]).is_err());
    }

    #[test]
    fn ou_scope_whitelist_blocks_outsiders() {
        let p = ResidencyPolicy::new(DataClassification::Aggregate)
            .permit_ou(uid("ouROOT00000"));
        assert!(p
            .check(DataClassification::Aggregate, &[uid("ouROOT00000")])
            .is_ok());
        let err = p
            .check(DataClassification::Aggregate, &[uid("ouOTHER0000")])
            .expect_err("outside scope");
        assert!(matches!(err, ResidencyViolation::OutsideOuScope { .. }));
    }
}
