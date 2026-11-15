//! Logical plan, binder, cost-based optimizer and EXPLAIN.
//!
//! The planner is the single entry point for turning a DER Semantic Query DSL document (or
//! DHIS2 semantic request) into a runnable physical pipeline. It owns provenance stitching
//! and policy injection; the runtime only executes what the planner approves.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod dsl;
pub mod explain;
pub mod plan;
pub mod pruning;
pub mod runner;

pub use dsl::{
    AggregateQuery, AggregationSpec, FilterSpec, OrderSpec, PeriodSpec, QueryConsistency,
    QueryRequest,
};
pub use explain::{ExplainPlan, OperatorNode};
pub use plan::{LogicalPlan, PhysicalPlan, PlanContext, PlanOptions};
pub use pruning::{prune_segments, segment_can_contribute};
pub use runner::{QueryExecutor, QueryResult};
