//! DAP core — typed value model, UIDs, DHIS2 periods, provenance and errors.
//!
//! Every other crate depends on these primitives. Keep this crate dependency-light and
//! zero-`unsafe`.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms, missing_debug_implementations)]

pub mod error;
pub mod uid;
pub mod value;
pub mod schema;
pub mod period;
pub mod provenance;
pub mod fingerprint;

pub use error::{DapError, DapResult};
pub use uid::Uid;
pub use value::{LogicalType, PhysicalType, Value};
pub use schema::{Column, Schema};
pub use period::{Period, PeriodType};
pub use provenance::{ConsistencyMode, Provenance, SiteCoverage};
pub use fingerprint::Fingerprint;
