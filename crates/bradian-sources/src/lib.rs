//! Generic data-source adapters for DAP.
//!
//! DAP is a general-purpose analytical engine. DHIS2 is one of many possible data sources;
//! this crate carries the format-specific readers that convert external inputs (CSV, JSON
//! Lines, Arrow IPC, SQL change-data-capture rows) into the canonical [`bradian_ingest::Change`]
//! envelope that the ingester already understands.
//!
//! Architectural layering (bottom-up):
//!
//! 1. **Core engine** — `bradian-core`, `bradian-storage`, `bradian-runtime`, `bradian-planner`,
//!    `bradian-views`, `bradian-serving`, `bradian-security`, `bradian-distributed`. Entirely
//!    domain-neutral: typed values, columnar segments, vectorized operators, scan + filter
//!    + aggregate + materialise, HTTP serving with authn/authz, federation coordinator.
//! 2. **Generic sources** — this crate. One module per transport / file-format, each
//!    producing `Change` envelopes.
//! 3. **Domain adapters** — `bradian-dhis2` (DHIS2 metadata + indicator semantics),
//!    `bradian-catalog::dhis2_adapter` (DHIS2 metadata JSON import). Additional adapters for
//!    OpenMRS, OpenIMIS, HAPI FHIR, Odoo, etc. plug in at the same seam.
//!
//! New domains need only (a) a catalog loader that translates their metadata to typed
//! [`Schema`](bradian_core::Schema) + lineage, and (b) a source adapter here (or a reuse of
//! one — most line-based systems expose CSV/JSONL already).
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod csv;
/// ADR 0304 — Bradian-native batch IPC. Not Apache Arrow
/// wire-compatible; see the ADR for rationale.
pub mod ipc;
pub mod jsonl;
pub mod sql_cdc;

pub use csv::{read_csv_bytes, CsvOptions};
pub use ipc::{read_ipc, write_ipc, IpcColumn, IpcRecordBatch, IpcValue, IpcValueType};
pub use jsonl::{read_jsonl, JsonlOptions};
pub use sql_cdc::{SqlCdcRow, SqlCdcSource};
