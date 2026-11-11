//! DHIS2 semantic compiler.
//!
//! v1 scope: program-indicator / indicator expressions over data element references
//! `#{DE_UID}` or `#{DE_UID.COC_UID}` with constants, `+ - * /` and unary minus. The parser
//! fails closed on any construct it does not recognise (spec §6.1: "Unsupported features
//! must fail with precise diagnostics rather than silent approximations").
//!
//! DHI-003 covers the versioned function matrix; v1 does not accept function calls — those
//! land once their reference parity fixtures are in place.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod expression;

pub use expression::{compile, evaluate, ExpressionAst};
