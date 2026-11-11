//! DHIS2 metadata types. Minimal viable surface for aggregate + event + tracker + indicator.
//!
//! These types stay narrow so the catalog can be hydrated from DHIS2's live API without
//! reaching into every corner of DHIS2's metadata universe. Fields the engine does not read
//! are omitted — add them as the semantic compiler needs them (DHI-* requirements).

use dap_core::{Fingerprint, Uid};
use serde::{Deserialize, Serialize};

/// A catalog revision is a BLAKE3-derived fingerprint of the serialised metadata snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MetadataRevision(pub Fingerprint);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetadataObject {
    DataElement(DataElement),
    Dataset(Dataset),
    CategoryCombo(CategoryCombo),
    CategoryOptionCombo(CategoryOptionCombo),
    OrgUnit(OrgUnit),
    Program(Program),
    ProgramStage(ProgramStage),
    ProgramIndicator(ProgramIndicator),
    Indicator(Indicator),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DataElement {
    pub uid: Uid,
    pub code: Option<String>,
    pub name: String,
    pub value_type: DataValueType,
    pub aggregation_type: AggregationType,
    pub category_combo: Uid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DataValueType {
    Number,
    Integer,
    PositiveInteger,
    NegativeInteger,
    ZeroOrPositiveInteger,
    Percentage,
    Boolean,
    Text,
    LongText,
    Date,
    DateTime,
    Time,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AggregationType {
    Sum,
    Average,
    AverageSumOrgUnit,
    Count,
    Min,
    Max,
    /// Population standard deviation.
    StdDev,
    /// Population variance.
    Variance,
    /// Latest value in a group, keyed by timestamp. For aggregate cells the key is the
    /// `(period, source_version)` tuple; for event / enrollment program indicators it is
    /// `event_date`.
    Last,
    /// Last value inside the reporting period (period axis uses LAST, OU axis averages).
    LastInPeriod,
    None,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Dataset {
    pub uid: Uid,
    pub name: String,
    pub period_type: dap_core::PeriodType,
    pub data_elements: Vec<Uid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CategoryCombo {
    pub uid: Uid,
    pub name: String,
    /// Associated option combos. The "default" category combo has exactly one.
    pub option_combos: Vec<Uid>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CategoryOptionCombo {
    pub uid: Uid,
    pub name: String,
    pub category_combo: Uid,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrgUnit {
    pub uid: Uid,
    pub code: Option<String>,
    pub name: String,
    pub parent: Option<Uid>,
    /// Depth of this unit in the hierarchy (root = 0).
    pub level: u16,
    /// Array of ancestor UIDs from root → self, inclusive. Mirrors DHIS2's `path` attribute.
    pub path: Vec<Uid>,
}

#[derive(Clone, Debug, Default)]
pub struct OrgUnitHierarchy {
    /// Child UIDs indexed by parent UID.
    pub children_of: ahash::AHashMap<Uid, Vec<Uid>>,
}

impl OrgUnitHierarchy {
    pub fn build(units: &[OrgUnit]) -> Self {
        let mut children_of: ahash::AHashMap<Uid, Vec<Uid>> = ahash::AHashMap::default();
        for u in units {
            if let Some(p) = &u.parent {
                children_of.entry(p.clone()).or_default().push(u.uid.clone());
            }
        }
        Self { children_of }
    }

    /// Iterator over the sub-tree rooted at `root`, inclusive, in pre-order.
    pub fn descendants(&self, root: &Uid) -> Vec<Uid> {
        let mut out = vec![root.clone()];
        let mut stack = vec![root.clone()];
        while let Some(n) = stack.pop() {
            if let Some(kids) = self.children_of.get(&n) {
                for k in kids {
                    out.push(k.clone());
                    stack.push(k.clone());
                }
            }
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Program {
    pub uid: Uid,
    pub name: String,
    pub program_type: ProgramType,
    pub stages: Vec<Uid>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProgramType {
    WithRegistration,
    WithoutRegistration,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramStage {
    pub uid: Uid,
    pub name: String,
    pub program: Uid,
    pub repeatable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProgramIndicator {
    pub uid: Uid,
    pub name: String,
    pub program: Uid,
    pub expression: String,
    #[serde(default)]
    pub filter: Option<String>,
    pub aggregation_type: AggregationType,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Indicator {
    pub uid: Uid,
    pub name: String,
    pub numerator: String,
    pub denominator: String,
    pub factor: i64,
}
