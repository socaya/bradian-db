//! Aggregate Program analytics.
//!
//! A DHIS2 aggregate "fact" is a single data value with coordinates
//! `(data_element, category_option_combo, attribute_option_combo, organisation_unit, period)`
//! and an optional numeric or textual value. Analytical queries roll these facts up across
//! one or more of those dimensions.
//!
//! The DHIS2 convention is that **each data element declares an `aggregationType`** which
//! controls how the engine combines values across *time* (period dimension) and *space*
//! (OU dimension). This module encodes the full matrix.
//!
//! Only the engine-relevant subset is implemented in v1:
//!
//! | DHIS2 aggregationType     | Across periods | Across org units | Notes |
//! |---------------------------|----------------|------------------|-------|
//! | `SUM`                     | SUM            | SUM              | default |
//! | `AVERAGE`                 | AVG            | AVG              | arithmetic mean of non-null values |
//! | `AVERAGE_SUM_ORG_UNIT`    | AVG            | SUM              | DHIS2 "average (sum in org unit hierarchy)" |
//! | `COUNT`                   | SUM            | SUM              | counts are additive |
//! | `MIN`                     | MIN            | MIN              | |
//! | `MAX`                     | MAX            | MAX              | |
//! | `NONE`                    | reject         | reject           | engine refuses to aggregate |
//! | `CUSTOM` / `DEFAULT`      | treated as SUM | treated as SUM   | with a warning in provenance |
//!
//! `LAST`, `LAST_AVERAGE_ORG_UNIT`, `LAST_IN_PERIOD`, `STDDEV`, `VARIANCE` are documented
//! here and tracked in `docs/gates/G6.md` as explicit gaps.

use dap_catalog::metadata::AggregationType;
use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

/// Reduction over a dimension.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reduction {
    Sum,
    Average,
    Count,
    Min,
    Max,
    /// Engine must not reduce along this axis; caller should surface per-cell values.
    None,
}

/// The reduction pair for a DHIS2 aggregation type: (across periods, across org units).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReductionPair {
    pub time: Reduction,
    pub org_unit: Reduction,
}

impl ReductionPair {
    pub fn from_aggregation_type(agg: AggregationType) -> DapResult<Self> {
        Ok(match agg {
            AggregationType::Sum | AggregationType::Custom | AggregationType::None => {
                ReductionPair {
                    time: if matches!(agg, AggregationType::None) {
                        Reduction::None
                    } else {
                        Reduction::Sum
                    },
                    org_unit: if matches!(agg, AggregationType::None) {
                        Reduction::None
                    } else {
                        Reduction::Sum
                    },
                }
            }
            AggregationType::Average => ReductionPair {
                time: Reduction::Average,
                org_unit: Reduction::Average,
            },
            AggregationType::AverageSumOrgUnit => ReductionPair {
                time: Reduction::Average,
                org_unit: Reduction::Sum,
            },
            AggregationType::Count => ReductionPair {
                time: Reduction::Sum,
                org_unit: Reduction::Sum,
            },
            AggregationType::Min => ReductionPair {
                time: Reduction::Min,
                org_unit: Reduction::Min,
            },
            AggregationType::Max => ReductionPair {
                time: Reduction::Max,
                org_unit: Reduction::Max,
            },
        })
    }

    pub fn is_additive_over_both_axes(&self) -> bool {
        matches!(
            (self.time, self.org_unit),
            (Reduction::Sum, Reduction::Sum) | (Reduction::Count, Reduction::Count)
        )
    }
}

/// Stateless combinator: apply a `Reduction` to a stream of `f64` values. `None` propagates
/// through `Min`/`Max`/`Average` such that they skip NULLs; `Reduction::None` panics because
/// the planner must not route a `None` series through the combinator.
#[derive(Debug, Default, Clone)]
pub struct Combiner {
    state: Option<State>,
}

#[derive(Debug, Clone)]
enum State {
    Sum(f64),
    Count(i64),
    Min(f64),
    Max(f64),
    Avg { sum: f64, count: i64 },
}

impl Combiner {
    pub fn ingest(&mut self, red: Reduction, v: Option<f64>) -> DapResult<()> {
        let v = match v {
            Some(v) => v,
            None => return Ok(()),
        };
        self.state = Some(match (red, self.state.clone()) {
            (Reduction::Sum, None) => State::Sum(v),
            (Reduction::Sum, Some(State::Sum(s))) => State::Sum(s + v),
            (Reduction::Count, None) => State::Count(1),
            (Reduction::Count, Some(State::Count(c))) => State::Count(c + 1),
            (Reduction::Min, None) => State::Min(v),
            (Reduction::Min, Some(State::Min(m))) => State::Min(m.min(v)),
            (Reduction::Max, None) => State::Max(v),
            (Reduction::Max, Some(State::Max(m))) => State::Max(m.max(v)),
            (Reduction::Average, None) => State::Avg { sum: v, count: 1 },
            (Reduction::Average, Some(State::Avg { sum, count })) => State::Avg {
                sum: sum + v,
                count: count + 1,
            },
            (Reduction::None, _) => {
                return Err(DapError::invariant(
                    "Reduction::None cannot be combined; planner must surface raw cells",
                ));
            }
            (red, Some(other)) => {
                return Err(DapError::invariant(format!(
                    "Reduction {red:?} does not match existing state {other:?}"
                )));
            }
        });
        Ok(())
    }

    pub fn finish(self) -> Option<f64> {
        match self.state {
            None => None,
            Some(State::Sum(v)) | Some(State::Min(v)) | Some(State::Max(v)) => Some(v),
            Some(State::Count(c)) => Some(c as f64),
            Some(State::Avg { sum, count }) if count > 0 => Some(sum / count as f64),
            Some(State::Avg { .. }) => None,
        }
    }
}

/// Collapse a cell-level table over the (period, org_unit) axes using the DE's aggregation
/// type. Input shape:
///   rows: Vec<(period, org_unit, value_opt)>  with all other dimensions already fixed.
/// Output: a single `Option<f64>`.
///
/// Semantics (DHIS2 reference behaviour):
///   1. Group by org_unit, reduce over periods using `pair.time`.
///   2. Reduce the resulting per-OU values over org units using `pair.org_unit`.
///
/// This is the canonical "time then OU" collapse DHIS2 documents for AVERAGE_SUM_ORG_UNIT
/// and friends. Calling with `pair.time == Reduction::None` returns an error — the planner
/// must inspect the aggregation type and surface raw cells instead.
pub fn collapse_cells(
    pair: ReductionPair,
    rows: impl IntoIterator<Item = (String, String, Option<f64>)>,
) -> DapResult<Option<f64>> {
    if matches!(pair.time, Reduction::None) || matches!(pair.org_unit, Reduction::None) {
        return Err(DapError::invalid_value(
            "aggregation type 'NONE' cannot collapse cells; planner must handle raw cells",
        ));
    }
    let mut by_ou: ahash::AHashMap<String, Combiner> = ahash::AHashMap::default();
    for (_period, ou, v) in rows {
        by_ou.entry(ou).or_default().ingest(pair.time, v)?;
    }
    let mut outer = Combiner::default();
    for (_ou, c) in by_ou {
        outer.ingest(pair.org_unit, c.finish())?;
    }
    Ok(outer.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cells() -> Vec<(String, String, Option<f64>)> {
        // Two org units, two periods each.
        vec![
            ("202601".into(), "ouA".into(), Some(10.0)),
            ("202602".into(), "ouA".into(), Some(30.0)),
            ("202601".into(), "ouB".into(), Some(20.0)),
            ("202602".into(), "ouB".into(), Some(40.0)),
        ]
    }

    #[test]
    fn sum_both_axes() {
        let p = ReductionPair::from_aggregation_type(AggregationType::Sum).unwrap();
        assert_eq!(collapse_cells(p, cells()).unwrap(), Some(100.0));
    }

    #[test]
    fn average_sum_org_unit_matches_dhis2_formula() {
        // DHIS2: AVERAGE_SUM_ORG_UNIT = sum across OUs of (average across periods per OU).
        // For ou A: avg(10, 30) = 20. For ou B: avg(20, 40) = 30. Sum = 50.
        let p =
            ReductionPair::from_aggregation_type(AggregationType::AverageSumOrgUnit).unwrap();
        assert_eq!(collapse_cells(p, cells()).unwrap(), Some(50.0));
    }

    #[test]
    fn average_both_axes() {
        // DHIS2: AVERAGE = avg across OUs of (avg across periods per OU).
        // A avg = 20. B avg = 30. Overall avg = 25.
        let p = ReductionPair::from_aggregation_type(AggregationType::Average).unwrap();
        assert_eq!(collapse_cells(p, cells()).unwrap(), Some(25.0));
    }

    #[test]
    fn min_max_both_axes() {
        let pmin = ReductionPair::from_aggregation_type(AggregationType::Min).unwrap();
        assert_eq!(collapse_cells(pmin, cells()).unwrap(), Some(10.0));
        let pmax = ReductionPair::from_aggregation_type(AggregationType::Max).unwrap();
        assert_eq!(collapse_cells(pmax, cells()).unwrap(), Some(40.0));
    }

    #[test]
    fn count_is_additive() {
        let p = ReductionPair::from_aggregation_type(AggregationType::Count).unwrap();
        assert!(p.is_additive_over_both_axes());
    }

    #[test]
    fn none_rejects_collapse() {
        let p = ReductionPair::from_aggregation_type(AggregationType::None).unwrap();
        assert!(collapse_cells(p, cells()).is_err());
    }

    #[test]
    fn null_values_are_skipped() {
        let p = ReductionPair::from_aggregation_type(AggregationType::Average).unwrap();
        let cells = vec![
            ("202601".into(), "ouA".into(), Some(10.0)),
            ("202602".into(), "ouA".into(), None),
            ("202601".into(), "ouB".into(), Some(20.0)),
            ("202602".into(), "ouB".into(), None),
        ];
        // Per-OU avgs: A = 10, B = 20. Mean = 15.
        assert_eq!(collapse_cells(p, cells).unwrap(), Some(15.0));
    }

    #[test]
    fn all_null_input_yields_none() {
        let p = ReductionPair::from_aggregation_type(AggregationType::Sum).unwrap();
        let cells = vec![
            ("202601".into(), "ouA".into(), None),
            ("202602".into(), "ouB".into(), None),
        ];
        assert_eq!(collapse_cells(p, cells).unwrap(), None);
    }
}
