//! Reproducible benchmark harness: synthetic DHIS2 fact generator + criterion wrappers.
//!
//! OPS-005. Numbers published without the full `BenchContext` (hardware, dataset size,
//! concurrency, cache state, commit SHA) are considered invalid per spec §12.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod fixtures;
pub mod report;

pub use fixtures::{generate_facts, generate_schema, FactsFixture};
pub use report::{BenchContext, BenchReport};
