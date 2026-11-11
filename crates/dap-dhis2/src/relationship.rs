//! Relationship analytics (DHI-004).
//!
//! A `RelationshipType` declares a kind of link between entities (TEI↔TEI, TEI↔enrollment,
//! TEI↔event, enrollment↔event, event↔event, bidirectional or directed). A `Relationship`
//! is one concrete instance of the type connecting two endpoints.
//!
//! This module provides:
//!
//! 1. Typed `Relationship` + endpoint model.
//! 2. `RelationshipIndex` keyed by each endpoint UID so `d2:relationshipCount` can answer in
//!    O(1) per query.
//! 3. A helper to turn a relationship count into a `V{relationship_count}` /
//!    `V{relationship_count:<typeUid>}` variable for the expression evaluator.

use ahash::{AHashMap, AHashSet};
use dap_core::Uid;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelationshipEndpointKind {
    TrackedEntity,
    Enrollment,
    Event,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationshipType {
    pub uid: Uid,
    pub name: String,
    pub from_kind: RelationshipEndpointKind,
    pub to_kind: RelationshipEndpointKind,
    /// If true, the relationship can be traversed from either side.
    pub bidirectional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RelationshipEndpoint {
    pub kind: RelationshipEndpointKind,
    /// UID of the endpoint entity (TEI / enrollment / event).
    pub entity: Uid,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Relationship {
    pub uid: Uid,
    pub relationship_type: Uid,
    pub from: RelationshipEndpoint,
    pub to: RelationshipEndpoint,
}

#[derive(Debug, Default)]
pub struct RelationshipIndex {
    types: AHashMap<Uid, RelationshipType>,
    all: Vec<Relationship>,
    by_entity: AHashMap<Uid, Vec<usize>>,
    by_entity_and_type: AHashMap<(Uid, Uid), Vec<usize>>,
}

impl RelationshipIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_type(&mut self, t: RelationshipType) {
        self.types.insert(t.uid.clone(), t);
    }

    pub fn insert(&mut self, r: Relationship) {
        let idx = self.all.len();
        let bidirectional = self
            .types
            .get(&r.relationship_type)
            .map(|t| t.bidirectional)
            .unwrap_or(false);
        for endpoint in [&r.from, &r.to] {
            self.by_entity
                .entry(endpoint.entity.clone())
                .or_default()
                .push(idx);
            self.by_entity_and_type
                .entry((endpoint.entity.clone(), r.relationship_type.clone()))
                .or_default()
                .push(idx);
            // For directed relationships, only record the "from" endpoint as traversable.
            if !bidirectional {
                break;
            }
        }
        self.all.push(r);
    }

    /// Count relationships touching `entity`. If `type_uid` is `Some`, restrict to that
    /// relationship type.
    pub fn count_for(&self, entity: &Uid, type_uid: Option<&Uid>) -> u32 {
        let indexes = match type_uid {
            Some(t) => self
                .by_entity_and_type
                .get(&(entity.clone(), t.clone()))
                .map(Vec::as_slice),
            None => self.by_entity.get(entity).map(Vec::as_slice),
        };
        indexes.map(|v| v.len() as u32).unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.all.len()
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// Return the set of distinct entity UIDs reachable from `entity` through relationships
    /// of the given type (or any type if `type_uid` is `None`). Used to compute cohort-style
    /// "who relates to this case" questions.
    pub fn neighbors(&self, entity: &Uid, type_uid: Option<&Uid>) -> Vec<Uid> {
        let mut seen: AHashSet<Uid> = AHashSet::default();
        let indexes = match type_uid {
            Some(t) => self
                .by_entity_and_type
                .get(&(entity.clone(), t.clone()))
                .cloned()
                .unwrap_or_default(),
            None => self.by_entity.get(entity).cloned().unwrap_or_default(),
        };
        for i in indexes {
            let r = &self.all[i];
            if r.from.entity == *entity {
                seen.insert(r.to.entity.clone());
            } else {
                seen.insert(r.from.entity.clone());
            }
        }
        seen.into_iter().collect()
    }
}

#[cfg(test)]
#[allow(clippy::useless_vec)]
mod tests {
    use super::*;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    fn rel(uid_s: &str, type_uid: &str, from: &str, to: &str) -> Relationship {
        Relationship {
            uid: uid(uid_s),
            relationship_type: uid(type_uid),
            from: RelationshipEndpoint {
                kind: RelationshipEndpointKind::TrackedEntity,
                entity: uid(from),
            },
            to: RelationshipEndpoint {
                kind: RelationshipEndpointKind::TrackedEntity,
                entity: uid(to),
            },
        }
    }

    fn setup() -> RelationshipIndex {
        let mut idx = RelationshipIndex::new();
        idx.register_type(RelationshipType {
            uid: uid("rtMOTHRCHLD"),
            name: "mother-child".into(),
            from_kind: RelationshipEndpointKind::TrackedEntity,
            to_kind: RelationshipEndpointKind::TrackedEntity,
            bidirectional: true,
        });
        idx.insert(rel("rel00000001", "rtMOTHRCHLD", "teMOTHER001", "teCHILD0001"));
        idx.insert(rel("rel00000002", "rtMOTHRCHLD", "teMOTHER001", "teCHILD0002"));
        idx.insert(rel("rel00000003", "rtMOTHRCHLD", "teMOTHER002", "teCHILD0003"));
        idx
    }

    #[test]
    fn count_for_entity_without_type() {
        let idx = setup();
        assert_eq!(idx.count_for(&uid("teMOTHER001"), None), 2);
        assert_eq!(idx.count_for(&uid("teCHILD0001"), None), 1);
        assert_eq!(idx.count_for(&uid("teOTHER0000"), None), 0);
    }

    #[test]
    fn count_for_entity_with_type_filter() {
        let idx = setup();
        assert_eq!(
            idx.count_for(&uid("teMOTHER001"), Some(&uid("rtMOTHRCHLD"))),
            2
        );
        // Unknown type filter → 0 matches.
        assert_eq!(
            idx.count_for(&uid("teMOTHER001"), Some(&uid("rtOTHER0000"))),
            0
        );
    }

    #[test]
    fn neighbors_dedup_and_follow_both_directions() {
        let idx = setup();
        let n = idx.neighbors(&uid("teMOTHER001"), None);
        assert_eq!(n.len(), 2);
        assert!(n.contains(&uid("teCHILD0001")));
        assert!(n.contains(&uid("teCHILD0002")));
    }
}
