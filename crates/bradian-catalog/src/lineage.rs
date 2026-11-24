//! Lineage/dependency graph linking metadata objects.
//!
//! CAT-002: "dependency and lineage graph; impact analysis on field change".

use bradian_core::Uid;
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

/// ADR 0299 — versioned lineage wrapper.
///
/// Each node carries a monotonic revision counter. Each
/// recorded edge captures the revisions of its endpoints
/// at creation time. An edge becomes "stale" when either
/// endpoint's current revision exceeds the recorded one,
/// signalling that downstream products derived from the
/// edge need to be re-validated.
#[derive(Clone, Debug, Default)]
pub struct VersionedLineage {
    graph:     Lineage,
    revisions: ahash::AHashMap<Uid, u64>,
    /// (edge, from_rev_at_record, to_rev_at_record)
    recorded:  Vec<(LineageEdge, u64, u64)>,
}

impl VersionedLineage {
    pub fn new() -> Self { Self::default() }

    /// Monotonically advance the node's revision counter.
    /// Returns the new revision.
    pub fn bump_revision(&mut self, uid: &Uid) -> u64 {
        let entry = self.revisions.entry(uid.clone()).or_insert(0);
        *entry += 1;
        *entry
    }

    pub fn revision(&self, uid: &Uid) -> u64 {
        *self.revisions.get(uid).unwrap_or(&0)
    }

    /// Record an edge against the endpoints' current
    /// revisions. If an endpoint has not been bumped yet,
    /// its revision is treated as 0 and initialised.
    pub fn record_edge(&mut self, edge: LineageEdge) {
        let fr = *self.revisions.entry(edge.from.clone()).or_insert(0);
        let tr = *self.revisions.entry(edge.to.clone()).or_insert(0);
        self.graph.add_edge(edge.clone());
        self.recorded.push((edge, fr, tr));
    }

    pub fn graph(&self) -> &Lineage { &self.graph }

    /// Edges whose endpoints have been bumped since the
    /// edge was recorded. Order follows record order so
    /// output is deterministic.
    pub fn stale_edges(&self) -> Vec<&LineageEdge> {
        self.recorded.iter()
            .filter(|(edge, from_rev, to_rev)| {
                self.revision(&edge.from) > *from_rev
                 || self.revision(&edge.to)   > *to_rev
            })
            .map(|(edge, _, _)| edge)
            .collect()
    }

    /// GraphViz DOT export. Stale edges are styled
    /// `[style=dashed,color=red]`; fresh edges carry the
    /// edge kind as a label.
    pub fn export_dot(&self) -> String {
        let stale: ahash::AHashSet<(Uid, Uid)> = self.stale_edges().iter()
            .map(|e| (e.from.clone(), e.to.clone())).collect();
        let mut out = String::from("digraph lineage {\n");
        // Deterministic node order.
        let mut nodes: Vec<&Uid> = self.revisions.keys().collect();
        nodes.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        for uid in nodes {
            let rev = self.revision(uid);
            out.push_str(&format!("  \"{uid}\" [label=\"{uid} @r{rev}\"];\n",
                uid = uid.as_ref()));
        }
        for (edge, _, _) in &self.recorded {
            let style = if stale.contains(&(edge.from.clone(), edge.to.clone())) {
                "[style=dashed,color=red]".to_string()
            } else {
                format!("[label=\"{:?}\"]", edge.kind)
            };
            out.push_str(&format!("  \"{}\" -> \"{}\" {style};\n",
                edge.from.as_ref(), edge.to.as_ref()));
        }
        out.push_str("}\n");
        out
    }

    /// JSON export. Shape:
    /// `{"nodes":[{"uid":...,"revision":N}],
    ///   "edges":[{"from":...,"to":...,"kind":"..."}],
    ///   "stale_edges":[{"from":...,"to":...}] }`.
    pub fn export_json(&self) -> String {
        use serde_json::json;
        let mut nodes: Vec<&Uid> = self.revisions.keys().collect();
        nodes.sort_by(|a, b| a.as_ref().cmp(b.as_ref()));
        let node_json: Vec<_> = nodes.iter()
            .map(|uid| json!({
                "uid": uid.as_ref(),
                "revision": self.revision(uid),
            })).collect();
        let edge_json: Vec<_> = self.recorded.iter()
            .map(|(edge, _, _)| json!({
                "from": edge.from.as_ref(),
                "to":   edge.to.as_ref(),
                "kind": format!("{:?}", edge.kind),
            })).collect();
        let stale_json: Vec<_> = self.stale_edges().iter()
            .map(|edge| json!({
                "from": edge.from.as_ref(),
                "to":   edge.to.as_ref(),
            })).collect();
        serde_json::to_string(&json!({
            "nodes":       node_json,
            "edges":       edge_json,
            "stale_edges": stale_json,
        })).expect("serialize versioned lineage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(s: &str) -> Uid { Uid::new(s).unwrap() }

    fn edge(from: &str, to: &str, kind: LineageKind) -> LineageEdge {
        LineageEdge { from: uid(from), to: uid(to), kind }
    }

    #[test]
    fn fresh_edges_are_not_stale_until_an_endpoint_is_bumped() {
        let mut lineage = VersionedLineage::new();
        lineage.record_edge(edge("DSaaaaaaaaa", "DEaaaaaaaaa",
            LineageKind::DatasetContainsDataElement));
        assert!(lineage.stale_edges().is_empty());
        // Bump the source → edge is now stale.
        lineage.bump_revision(&uid("DSaaaaaaaaa"));
        let stale = lineage.stale_edges();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].from.as_ref(), "DSaaaaaaaaa");
    }

    #[test]
    fn bumping_target_marks_only_edges_pointing_to_that_node() {
        let mut lineage = VersionedLineage::new();
        lineage.record_edge(edge("DSaaaaaaaaa", "DEaaaaaaaaa",
            LineageKind::DatasetContainsDataElement));
        lineage.record_edge(edge("DSaaaaaaaaa", "DEbbbbbbbbb",
            LineageKind::DatasetContainsDataElement));
        lineage.bump_revision(&uid("DEaaaaaaaaa"));
        let stale = lineage.stale_edges();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].to.as_ref(), "DEaaaaaaaaa");
    }

    #[test]
    fn recording_edge_after_bump_captures_new_revision_as_fresh() {
        let mut lineage = VersionedLineage::new();
        lineage.bump_revision(&uid("DSaaaaaaaaa"));
        lineage.record_edge(edge("DSaaaaaaaaa", "DEaaaaaaaaa",
            LineageKind::DatasetContainsDataElement));
        assert!(lineage.stale_edges().is_empty(),
            "edge recorded AFTER bump should be fresh");
        // Second bump makes it stale.
        lineage.bump_revision(&uid("DSaaaaaaaaa"));
        assert_eq!(lineage.stale_edges().len(), 1);
    }

    #[test]
    fn dot_and_json_exports_mark_stale_edges_distinctly() {
        let mut lineage = VersionedLineage::new();
        lineage.record_edge(edge("DSaaaaaaaaa", "DEaaaaaaaaa",
            LineageKind::DatasetContainsDataElement));
        lineage.record_edge(edge("DSaaaaaaaaa", "DEbbbbbbbbb",
            LineageKind::DatasetContainsDataElement));
        lineage.bump_revision(&uid("DEaaaaaaaaa"));
        let dot = lineage.export_dot();
        assert!(dot.contains("digraph lineage"));
        assert!(dot.contains("\"DSaaaaaaaaa\" -> \"DEaaaaaaaaa\" [style=dashed,color=red]"),
            "stale edge missing dashed-red marker: {dot}");
        assert!(dot.contains("\"DSaaaaaaaaa\" -> \"DEbbbbbbbbb\" [label="),
            "fresh edge missing labeled marker: {dot}");

        let parsed: serde_json::Value = serde_json::from_str(&lineage.export_json()).unwrap();
        assert_eq!(parsed["nodes"].as_array().unwrap().len(), 3);
        assert_eq!(parsed["edges"].as_array().unwrap().len(), 2);
        let stale = parsed["stale_edges"].as_array().unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0]["to"], "DEaaaaaaaaa");
    }
}
