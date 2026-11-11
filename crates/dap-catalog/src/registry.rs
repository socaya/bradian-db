//! In-memory versioned catalog registry.

use std::sync::Arc;

use dap_core::{DapError, DapResult, Fingerprint, PeriodType, Uid};
use parking_lot::RwLock;

use crate::lineage::{Lineage, LineageEdge, LineageKind};
use crate::metadata::{
    CategoryCombo, CategoryOptionCombo, DataElement, Dataset, Indicator, MetadataObject,
    OrgUnit, OrgUnitHierarchy, Program, ProgramIndicator, ProgramStage,
};

#[derive(Default, Debug)]
struct Inner {
    data_elements: ahash::AHashMap<Uid, DataElement>,
    datasets: ahash::AHashMap<Uid, Dataset>,
    category_combos: ahash::AHashMap<Uid, CategoryCombo>,
    option_combos: ahash::AHashMap<Uid, CategoryOptionCombo>,
    org_units: ahash::AHashMap<Uid, OrgUnit>,
    programs: ahash::AHashMap<Uid, Program>,
    program_stages: ahash::AHashMap<Uid, ProgramStage>,
    program_indicators: ahash::AHashMap<Uid, ProgramIndicator>,
    indicators: ahash::AHashMap<Uid, Indicator>,
    org_unit_hierarchy: OrgUnitHierarchy,
    lineage: Lineage,
    revision: Option<Fingerprint>,
}

#[derive(Debug, Default)]
pub struct Catalog {
    inner: RwLock<Inner>,
}

impl Catalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn into_arc(self) -> Arc<Self> {
        Arc::new(self)
    }

    pub fn revision(&self) -> Fingerprint {
        self.inner.read().revision.unwrap_or_else(|| Fingerprint::of(b"empty-catalog"))
    }

    /// Bulk-apply a list of metadata objects, replacing any object with the same UID. A single
    /// atomic revision bump is emitted at the end.
    pub fn apply(&self, objects: Vec<MetadataObject>) -> DapResult<Fingerprint> {
        let mut inner = self.inner.write();
        for obj in objects {
            match obj {
                MetadataObject::DataElement(de) => {
                    inner
                        .lineage
                        .add_edge(LineageEdge {
                            from: de.uid.clone(),
                            to: de.category_combo.clone(),
                            kind: LineageKind::DataElementUsesCategoryCombo,
                        });
                    inner.data_elements.insert(de.uid.clone(), de);
                }
                MetadataObject::Dataset(ds) => {
                    for de in &ds.data_elements {
                        inner.lineage.add_edge(LineageEdge {
                            from: ds.uid.clone(),
                            to: de.clone(),
                            kind: LineageKind::DatasetContainsDataElement,
                        });
                    }
                    inner.datasets.insert(ds.uid.clone(), ds);
                }
                MetadataObject::CategoryCombo(cc) => {
                    for coc in &cc.option_combos {
                        inner.lineage.add_edge(LineageEdge {
                            from: coc.clone(),
                            to: cc.uid.clone(),
                            kind: LineageKind::OptionComboOfCategoryCombo,
                        });
                    }
                    inner.category_combos.insert(cc.uid.clone(), cc);
                }
                MetadataObject::CategoryOptionCombo(coc) => {
                    inner.option_combos.insert(coc.uid.clone(), coc);
                }
                MetadataObject::OrgUnit(ou) => {
                    if let Some(p) = &ou.parent {
                        inner.lineage.add_edge(LineageEdge {
                            from: ou.uid.clone(),
                            to: p.clone(),
                            kind: LineageKind::OrgUnitParent,
                        });
                    }
                    inner.org_units.insert(ou.uid.clone(), ou);
                }
                MetadataObject::Program(p) => {
                    for s in &p.stages {
                        inner.lineage.add_edge(LineageEdge {
                            from: p.uid.clone(),
                            to: s.clone(),
                            kind: LineageKind::ProgramHasStage,
                        });
                    }
                    inner.programs.insert(p.uid.clone(), p);
                }
                MetadataObject::ProgramStage(s) => {
                    inner.program_stages.insert(s.uid.clone(), s);
                }
                MetadataObject::ProgramIndicator(pi) => {
                    inner.lineage.add_edge(LineageEdge {
                        from: pi.uid.clone(),
                        to: pi.program.clone(),
                        kind: LineageKind::ProgramIndicatorDependsOn,
                    });
                    inner.program_indicators.insert(pi.uid.clone(), pi);
                }
                MetadataObject::Indicator(i) => {
                    inner.indicators.insert(i.uid.clone(), i);
                }
            }
        }
        // Rebuild org unit hierarchy from scratch.
        inner.org_unit_hierarchy = OrgUnitHierarchy::build(
            &inner.org_units.values().cloned().collect::<Vec<_>>(),
        );
        // Fingerprint by the UID set + their serialised representation. Deterministic ordering
        // uses the UID in sorted order.
        let mut uids: Vec<_> = inner.data_elements.keys().cloned().collect();
        uids.extend(inner.datasets.keys().cloned());
        uids.extend(inner.category_combos.keys().cloned());
        uids.extend(inner.option_combos.keys().cloned());
        uids.extend(inner.org_units.keys().cloned());
        uids.extend(inner.programs.keys().cloned());
        uids.extend(inner.program_stages.keys().cloned());
        uids.extend(inner.program_indicators.keys().cloned());
        uids.extend(inner.indicators.keys().cloned());
        uids.sort();
        let mut hasher_input = Vec::new();
        for u in uids {
            hasher_input.extend_from_slice(u.as_str().as_bytes());
            hasher_input.push(0);
        }
        let revision = Fingerprint::of(&hasher_input);
        inner.revision = Some(revision);
        Ok(revision)
    }

    pub fn data_element(&self, uid: &Uid) -> DapResult<DataElement> {
        self.inner
            .read()
            .data_elements
            .get(uid)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("data element {uid}")))
    }

    pub fn dataset(&self, uid: &Uid) -> DapResult<Dataset> {
        self.inner
            .read()
            .datasets
            .get(uid)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("dataset {uid}")))
    }

    pub fn org_unit(&self, uid: &Uid) -> DapResult<OrgUnit> {
        self.inner
            .read()
            .org_units
            .get(uid)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("org unit {uid}")))
    }

    pub fn program(&self, uid: &Uid) -> DapResult<Program> {
        self.inner
            .read()
            .programs
            .get(uid)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("program {uid}")))
    }

    pub fn program_indicator(&self, uid: &Uid) -> DapResult<ProgramIndicator> {
        self.inner
            .read()
            .program_indicators
            .get(uid)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("program indicator {uid}")))
    }

    pub fn indicator(&self, uid: &Uid) -> DapResult<Indicator> {
        self.inner
            .read()
            .indicators
            .get(uid)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("indicator {uid}")))
    }

    /// Descendant org units of `root`, inclusive.
    pub fn org_unit_subtree(&self, root: &Uid) -> Vec<Uid> {
        self.inner.read().org_unit_hierarchy.descendants(root)
    }

    pub fn period_type_of_dataset(&self, uid: &Uid) -> DapResult<PeriodType> {
        Ok(self.dataset(uid)?.period_type)
    }

    /// UIDs that depend on `uid` transitively. Used to invalidate materialized views when
    /// metadata changes.
    pub fn transitive_dependents(&self, uid: &Uid) -> Vec<Uid> {
        self.inner.read().lineage.transitive_dependents(uid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    #[test]
    fn org_unit_subtree() {
        let cat = Catalog::new();
        let objs = vec![
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouROOT00000"),
                code: None,
                name: "root".into(),
                parent: None,
                level: 0,
                path: vec![uid("ouROOT00000")],
            }),
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouChildAAAA"),
                code: None,
                name: "A".into(),
                parent: Some(uid("ouROOT00000")),
                level: 1,
                path: vec![uid("ouROOT00000"), uid("ouChildAAAA")],
            }),
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouChildBBBB"),
                code: None,
                name: "B".into(),
                parent: Some(uid("ouROOT00000")),
                level: 1,
                path: vec![uid("ouROOT00000"), uid("ouChildBBBB")],
            }),
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouLeafAAAAA"),
                code: None,
                name: "A1".into(),
                parent: Some(uid("ouChildAAAA")),
                level: 2,
                path: vec![uid("ouROOT00000"), uid("ouChildAAAA"), uid("ouLeafAAAAA")],
            }),
        ];
        cat.apply(objs).unwrap();

        let desc = cat.org_unit_subtree(&uid("ouROOT00000"));
        assert_eq!(desc.len(), 4);

        let desc_child = cat.org_unit_subtree(&uid("ouChildAAAA"));
        assert_eq!(desc_child.len(), 2);
        assert!(desc_child.contains(&uid("ouLeafAAAAA")));
    }

    #[test]
    fn dataset_lookup_and_period_type() {
        let cat = Catalog::new();
        cat.apply(vec![MetadataObject::Dataset(Dataset {
            uid: uid("dsAAAAAAAAA"),
            name: "D".into(),
            period_type: PeriodType::Monthly,
            data_elements: vec![uid("deAAAAAAAAA")],
        })])
        .unwrap();
        assert_eq!(
            cat.period_type_of_dataset(&uid("dsAAAAAAAAA")).unwrap(),
            PeriodType::Monthly
        );
    }

    #[test]
    fn lineage_tracks_dataset_to_dataelement() {
        let cat = Catalog::new();
        cat.apply(vec![
            MetadataObject::Dataset(Dataset {
                uid: uid("dsAAAAAAAAA"),
                name: "D".into(),
                period_type: PeriodType::Monthly,
                data_elements: vec![uid("deAAAAAAAAA")],
            }),
        ])
        .unwrap();
        let deps = cat.transitive_dependents(&uid("deAAAAAAAAA"));
        assert!(deps.contains(&uid("dsAAAAAAAAA")));
    }
}
