//! Dependency DAG linking sources (data elements, periods, OU subtrees, program stages) to
//! downstream projections, indicators, materialized views and data products.
//!
//! The DAG lives in memory and is populated at view-registration time. Invalidation walks
//! the reverse graph to find every downstream node affected by a change.

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    SourceColumn(String),
    DataElement(dap_core::Uid),
    ProgramIndicator(dap_core::Uid),
    MaterializedView(String),
    DataProduct(String),
}

#[derive(Debug)]
pub struct DependencyDag {
    inner: RwLock<Inner>,
}

#[derive(Debug, Default)]
struct Inner {
    nodes: Vec<NodeKind>,
    edges_from: ahash::AHashMap<NodeId, Vec<NodeId>>,
    edges_to: ahash::AHashMap<NodeId, Vec<NodeId>>,
}

impl DependencyDag {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(Inner::default()),
        }
    }

    pub fn add_node(&self, kind: NodeKind) -> NodeId {
        let mut inner = self.inner.write();
        let id = NodeId(inner.nodes.len() as u32);
        inner.nodes.push(kind);
        id
    }

    pub fn add_edge(&self, from: NodeId, to: NodeId) {
        let mut inner = self.inner.write();
        inner.edges_from.entry(from).or_default().push(to);
        inner.edges_to.entry(to).or_default().push(from);
    }

    /// Transitive downstream nodes of `root` (nodes that read from root directly or
    /// transitively).
    pub fn downstream(&self, root: NodeId) -> Vec<NodeId> {
        let inner = self.inner.read();
        let mut seen: ahash::AHashSet<NodeId> = Default::default();
        let mut stack = vec![root];
        let mut out = Vec::new();
        while let Some(n) = stack.pop() {
            if let Some(kids) = inner.edges_from.get(&n) {
                for &k in kids {
                    if seen.insert(k) {
                        out.push(k);
                        stack.push(k);
                    }
                }
            }
        }
        out
    }

    pub fn kind(&self, id: NodeId) -> Option<NodeKind> {
        self.inner.read().nodes.get(id.0 as usize).cloned()
    }
}

impl Default for DependencyDag {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downstream_walk() {
        let dag = DependencyDag::new();
        let de = dag.add_node(NodeKind::SourceColumn("value".into()));
        let pi = dag.add_node(NodeKind::ProgramIndicator(dap_core::Uid::new("piAAAAAAAAA").unwrap()));
        let mv = dag.add_node(NodeKind::MaterializedView("mv_facility".into()));
        let dp = dag.add_node(NodeKind::DataProduct("dp_region".into()));

        dag.add_edge(de, pi);
        dag.add_edge(pi, mv);
        dag.add_edge(mv, dp);

        let reach = dag.downstream(de);
        assert_eq!(reach.len(), 3);
    }
}
