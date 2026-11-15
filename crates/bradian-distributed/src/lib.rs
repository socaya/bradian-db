//! Distributed analytical fabric for DAP (spec §9, requirements DST-001..004).
//!
//! The distributed layer operates in four modes (spec §9.1):
//!
//! - **Site-local** — a single edge partition answers queries without reaching the WAN.
//! - **Server-consolidated** — the central server answers from already-acknowledged state.
//! - **Federated** — a coordinator plans a query, splits it into fragments, dispatches to
//!   the authorised workers, and merges partial aggregate state.
//! - **Hybrid** — edge contributions sit on top of central state, with explicit "fresh-
//!   but-unacknowledged" markers in provenance.
//!
//! The scope here is correctness-first: an in-process coordinator that routes fragments
//! across any `FragmentExecutor` implementation. A network transport is a later gate; the
//! HTTP serving layer can plug a remote executor in via the same trait.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod coordinator;
pub mod fragment;
pub mod reconcile;
pub mod residency;
pub mod worker;

pub use coordinator::{Coordinator, FederatedResult};
pub use fragment::{Fragment, FragmentKind, FragmentResult};
pub use reconcile::{ReconciliationLog, ReconciliationOutcome, SiteCheckpoint};
pub use residency::{DataClassification, ResidencyPolicy, ResidencyViolation};
pub use worker::{
    FragmentExecutor, InProcessWorker, PlanLoader, WorkerCapabilities, WorkerId, WorkerRegistry,
};
