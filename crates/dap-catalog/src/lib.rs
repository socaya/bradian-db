//! Versioned DHIS2 semantic catalog.
//!
//! Spec §4 + requirements CAT-001/002/003. The catalog is in-process state owned by the
//! server — not persisted here. A separate migration layer loads it from DHIS2 metadata
//! endpoints, YAML seed files or snapshots.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod dhis2_adapter;
pub mod lineage;
pub mod metadata;
pub mod registry;

pub use dhis2_adapter::{apply_dhis2_bundle, apply_dhis2_metadata_json, Dhis2MetadataBundle};
pub use lineage::{Lineage, LineageEdge, LineageKind};
pub use metadata::{
    CategoryCombo, CategoryOptionCombo, DataElement, Dataset, Indicator, MetadataObject,
    MetadataRevision, OrgUnit, OrgUnitHierarchy, Program, ProgramIndicator, ProgramStage,
};
pub use registry::Catalog;
