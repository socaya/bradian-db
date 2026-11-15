//! Program rule side-effect analytics.
//!
//! DHIS2 Program Rules can generate derived data-element values during data entry
//! ("assign", "showWarning", "setMandatoryField", etc.). From the analytical engine's
//! viewpoint the only visible side effect is an *assigned data-element value*: the
//! transactional system stores the computed value in the same table as a user-entered one.
//! DAP must:
//!
//! 1. **Preserve provenance.** Every `EventFact` / aggregate fact can carry a
//!    [`RuleSource`] stamp so operators can tell whether a value was entered manually,
//!    computed by a rule, or imported from an external system.
//! 2. **Not re-evaluate the rule.** The rule engine lives in DHIS2's capture layer; the
//!    analytical engine treats the stored value as truth.
//! 3. **Flag inconsistency.** When a rule-generated value is later edited manually, the
//!    newer source version wins per the ingest version-fence rule (ING-002); the engine
//!    surfaces a provenance note so operators can audit the override.
//!
//! The provenance stamp is carried in the canonical change envelope's `metadata_revision`
//! field together with an explicit `RuleSource` field that the ingester plumbs through.
//! This module provides the typed value and a tiny helper to project it to a human-readable
//! note.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuleSource {
    /// Captured by a user form.
    DataEntry,
    /// Computed by a DHIS2 program rule ("assign" action).
    ProgramRule,
    /// Loaded through the bulk import API.
    Import,
    /// Derived by an analytical materialized view.
    Derived,
    /// Externally reconciled (e.g. HMIS ingest).
    External,
}

impl RuleSource {
    pub fn is_user_authored(&self) -> bool {
        matches!(self, RuleSource::DataEntry)
    }

    pub fn is_machine_generated(&self) -> bool {
        matches!(
            self,
            RuleSource::ProgramRule | RuleSource::Derived | RuleSource::Import
        )
    }

    pub fn provenance_note(&self) -> &'static str {
        match self {
            RuleSource::DataEntry => "value entered by a user via capture",
            RuleSource::ProgramRule => "value computed by a program rule",
            RuleSource::Import => "value loaded via bulk import",
            RuleSource::Derived => "value derived by an analytics materialized view",
            RuleSource::External => "value loaded from an external reconciliation",
        }
    }
}

/// A projection the ingester attaches to the canonical change payload so the storage layer
/// can preserve it. Not a replacement for the full DHIS2 Program Rule execution model.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuleProvenance {
    pub source: RuleSource,
    /// Optional program-rule UID that produced the value. Only set when
    /// `source == ProgramRule`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_uid: Option<String>,
}

impl RuleProvenance {
    pub fn data_entry() -> Self {
        Self {
            source: RuleSource::DataEntry,
            rule_uid: None,
        }
    }

    pub fn from_rule(rule_uid: impl Into<String>) -> Self {
        Self {
            source: RuleSource::ProgramRule,
            rule_uid: Some(rule_uid.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_vs_machine_distinction() {
        assert!(RuleSource::DataEntry.is_user_authored());
        assert!(!RuleSource::ProgramRule.is_user_authored());
        assert!(RuleSource::ProgramRule.is_machine_generated());
        assert!(!RuleSource::External.is_user_authored());
    }

    #[test]
    fn note_for_each_source() {
        for s in [
            RuleSource::DataEntry,
            RuleSource::ProgramRule,
            RuleSource::Import,
            RuleSource::Derived,
            RuleSource::External,
        ] {
            assert!(!s.provenance_note().is_empty());
        }
    }
}
