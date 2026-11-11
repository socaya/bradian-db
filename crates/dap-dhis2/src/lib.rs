//! DHIS2 semantic compiler.
//!
//! Coverage
//! ========
//!
//! DAP implements the three analytical families DHIS2 exposes:
//!
//! 1. **Aggregate program analytics** — classic data value tables
//!    `(DE, COC, AOC, OU, period) -> value`. The engine honours each data element's
//!    `aggregationType` (SUM / AVERAGE / AVERAGE_SUM_ORG_UNIT / COUNT / MIN / MAX / …)
//!    when rolling up across the period and org-unit dimensions. See [`aggregate`].
//! 2. **Classic Indicators** — `(numerator, denominator, factor)` ratios over aggregate
//!    data values, with SQL null propagation and divide-by-zero → NULL. See [`indicator`].
//! 3. **Program Indicators** for Event and Tracker programs — analytics types
//!    [`AnalyticsType::Event`](event::AnalyticsType::Event) and
//!    [`AnalyticsType::Enrollment`](event::AnalyticsType::Enrollment). Both evaluate the
//!    same expression grammar against the matching fact grain. Tracker-grain evaluation
//!    de-duplicates TEIs automatically and aggregates repeatable-stage values with the
//!    expression author's chosen aggregation type. See [`event`] and [`tracker`].
//!
//! The expression grammar ([`expression`]) is shared by all three paths. It supports
//! references (`#{DE}` / `#{DE.COC}` / `#{DE.COC.AOC}`, `C{uid}`, `V{name}`, `I{uid}`,
//! `A{attr}` / `A{program.attr}`, `D{stage.de}`, `[days]`), literals (numbers, strings,
//! `true`, `false`, `null`), unary `+ - !`, binary `* / % + - < <= > >= == != && ||`, and
//! the whitelisted function set declared in [`SUPPORTED_FUNCTIONS`]. Any construct outside
//! that whitelist fails closed with a precise diagnostic (spec §6.1).
//!
//! Unimplemented DHIS2 analytical features are tracked in `docs/gates/G6.md`.
#![deny(unsafe_code)]
#![warn(rust_2018_idioms)]

pub mod aggregate;
pub mod completion;
pub mod event;
pub mod expression;
pub mod indicator;
pub mod program_rule;
pub mod relationship;
pub mod tracker;

pub use aggregate::{collapse_cells, collapse_cells_ts, Combiner, Reduction, ReductionPair};
pub use completion::{CompletionFact, CompletionKey, CompletionStore};
pub use event::{
    evaluate_event_program_indicator, AnalyticsType, EventFact, EventStatus, PIResult,
    ProgramIndicatorPlan,
};
pub use expression::{
    compile, evaluate, evaluate_expr, validate_functions, EValue, ExpressionAst, RefKind,
    ReportingRateVariant, Resolver, SUPPORTED_FUNCTIONS,
};
pub use indicator::{compile_indicator, evaluate_indicator, IndicatorPlan, IndicatorResolver};
pub use program_rule::{RuleProvenance, RuleSource};
pub use relationship::{
    Relationship, RelationshipEndpoint, RelationshipEndpointKind, RelationshipIndex,
    RelationshipType,
};
pub use tracker::{
    completions_in_period, distinct_tei_count, enrollments_in_period, events_by_enrollment,
    evaluate_enrollment_program_indicator, Enrollment, EnrollmentContext, EnrollmentResolver,
    EnrollmentStatus, TrackedEntity,
};
