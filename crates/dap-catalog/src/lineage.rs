//! Lineage/dependency graph linking metadata objects.
//!
//! CAT-002: "dependency and lineage graph; impact analysis on field change".

use dap_core::Uid;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum LineageKind {
    /// Dataset contains DataElement.
    DatasetContainsDataElement,
    /// DataElement uses CategoryCombo.
    DataElementUsesCategoryCombo,
    /// CategoryOptionCombo belongs to CategoryCombo.
    OptionComboOfCategoryCombo,
    /// OrgUnit hierarchy parent/child.
    OrgUnitParent,
    /// Program references ProgramStage.
    ProgramHasStage,
    /// ProgramIndicator references Program + DataElement.
    ProgramIndicatorDependsOn,
    /// Indicator numerator / denominator depends on DataElement.
    IndicatorDependsOn,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LineageEdge {
    pub from: Uid,
    pub to: Uid,
    pub kind: LineageKind,
}

/// Directed graph with edges keyed by source UID. Not thread-safe; mutation flows through the
/// catalog's `Registry` write lock.
#[derive(Clone, Debug, Default)]
pub struct Lineage {
    edges_from: ahash::AHashMap<Uid, Vec<LineageEdge>>,
    edges_to: ahash::AHashMap<Uid, Vec<LineageEdge>>,
}

impl Lineage {
    pub fn add_edge(&mut self, edge: LineageEdge) {
        self.edges_from
            .entry(edge.from.clone())
            .or_default()
            .push(edge.clone());
        self.edges_to.entry(edge.to.clone()).or_default().push(edge);
    }

    pub fn outbound(&self, from: &Uid) -> impl Iterator<Item = &LineageEdge> {
        self.edges_from
            .get(from)
            .into_iter()
            .flat_map(|v| v.iter())
    }

    pub fn inbound(&self, to: &Uid) -> impl Iterator<Item = &LineageEdge> {
        self.edges_to
            .get(to)
            .into_iter()
            .flat_map(|v| v.iter())
    }

    /// Transitive closure of upstream dependents of `uid`. Used when a metadata field change
    /// should invalidate every downstream materialized view / product.
    pub fn transitive_dependents(&self, uid: &Uid) -> Vec<Uid> {
        let mut seen: ahash::AHashSet<Uid> = Default::default();
        let mut stack = vec![uid.clone()];
        let mut out = Vec::new();
        while let Some(n) = stack.pop() {
            for e in self.inbound(&n) {
                if seen.insert(e.from.clone()) {
                    out.push(e.from.clone());
                    stack.push(e.from.clone());
                }
            }
        }
        out
    }
}
