//! Canonical change log + idempotent ingestion.
//!
//! Spec §7.1. The canonical envelope is the on-wire shape that external adapters (DHIS2
//! bulk/API, PostgreSQL CDC, test fixtures) convert their input into. The ingestion layer
//! applies the envelope to a `Partition` with the dataset-specific primary key, using
//! (objectUid, sourceVersion) as the version fence.
//!
//! Requirements: ING-001/002/003.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod envelope;
pub mod ingester;
pub mod quarantine;

pub use envelope::{Change, Operation};
pub use ingester::{IngestReport, Ingester};
pub use quarantine::QuarantineLog;
