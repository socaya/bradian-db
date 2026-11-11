//! DHIS2 semantic compiler.
//!
//! Scope
//! =====
//!
//! DHIS2 has two distinct indicator families and DAP compiles both. They share the
//! `#{DE_UID}` / `#{DE_UID.COC_UID}` reference grammar and the `+ - * /` operator set, but
//! differ in what they aggregate over and how they combine primitive results:
//!
//! - **Program Indicators** (DHI-003): single-expression formulas that operate over tracker
//!   or event fact tables. See [`expression`].
//! - **Indicators** (DHI-003 + DHI-001): numerator / denominator formulas over aggregate
//!   data values, applied with an integer factor (typically 1, 100, 1 000 or 100 000). See
//!   [`indicator`]. A compiled `IndicatorPlan` evaluates `(numerator / denominator) * factor`
//!   with SQL null propagation and divide-by-zero -> NULL.
//!
//! The parser fails closed on any construct it does not recognise (spec §6.1: "Unsupported
//! features must fail with precise diagnostics rather than silent approximations"). Unknown
//! function calls, `C{...}` constants, `[days]`, `V{...}` variables and the dataset
//! `REPORTING_RATE` / `ACTUAL_REPORTS` variants are explicitly rejected until their parity
//! fixtures land.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod expression;
pub mod indicator;

pub use expression::{compile, evaluate, ExpressionAst};
pub use indicator::{compile_indicator, evaluate_indicator, IndicatorPlan, IndicatorResolver};
