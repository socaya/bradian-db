//! Aggregate Program analytics.
//!
//! A DHIS2 aggregate "fact" is a single data value with coordinates
//! `(data_element, category_option_combo, attribute_option_combo, organisation_unit, period)`
//! and an optional numeric or textual value. Analytical queries roll these facts up across
//! one or more of those dimensions.
//!
//! The DHIS2 convention is that **each data element declares an `aggregationType`** which
//! controls how the engine combines values across *time* (period dimension) and *space*
//! (OU dimension). This module encodes the full matrix:
//!
//! | DHIS2 aggregationType     | Across periods | Across org units | Notes |
//! |---------------------------|----------------|------------------|-------|
//! | `SUM`                     | SUM            | SUM              | default |
//! | `AVERAGE`                 | AVG            | AVG              | arithmetic mean of non-null values |
//! | `AVERAGE_SUM_ORG_UNIT`    | AVG            | SUM              | DHIS2 "average (sum in org unit hierarchy)" |
//! | `COUNT`                   | SUM            | SUM              | counts are additive |
//! | `MIN`                     | MIN            | MIN              | |
//! | `MAX`                     | MAX            | MAX              | |
//! | `STDDEV`                  | STDDEV         | STDDEV           | Welford population stddev |
//! | `VARIANCE`                | VARIANCE       | VARIANCE         | Welford population variance |
//! | `LAST`                    | LAST           | LAST             | pick-by-timestamp (requires `collapse_cells_ts`) |
//! | `LAST_IN_PERIOD`          | LAST           | AVERAGE          | pick-by-timestamp within a period then average across OUs |
//! | `NONE`                    | reject         | reject           | engine refuses to aggregate |
//! | `CUSTOM`                  | treated as SUM | treated as SUM   | provenance note surfaces the override |

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
    /// Welford population standard deviation.
    StdDev,
    /// Welford population variance.
    Variance,
    /// Pick-by-timestamp; see `collapse_cells_ts`. Requires the caller to thread a
    /// companion ordering key per cell; plain `ingest` on a `Last` reduction uses the
    /// insertion order as a fallback.
    Last,
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
            AggregationType::Sum | AggregationType::Custom => ReductionPair {
                time: Reduction::Sum,
                org_unit: Reduction::Sum,
            },
            AggregationType::None => ReductionPair {
                time: Reduction::None,
                org_unit: Reduction::None,
            },
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
            AggregationType::StdDev => ReductionPair {
                time: Reduction::StdDev,
                org_unit: Reduction::StdDev,
            },
            AggregationType::Variance => ReductionPair {
                time: Reduction::Variance,
                org_unit: Reduction::Variance,
            },
            AggregationType::Last => ReductionPair {
                time: Reduction::Last,
                org_unit: Reduction::Last,
            },
            AggregationType::LastInPeriod => ReductionPair {
                time: Reduction::Last,
                org_unit: Reduction::Average,
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

/// Stateless combinator: apply a `Reduction` to a stream of `f64` values. `None` values are
/// skipped (SQL NULL semantics).
///
/// For `Reduction::Last`, call `ingest_timestamped` with the companion ordering key.
#[derive(Debug, Default, Clone)]
pub struct Combiner {
    state: Option<State>,
    terminal: Option<Reduction>,
}

#[derive(Debug, Clone)]
enum State {
    Sum(f64),
    Count(i64),
    Min(f64),
    Max(f64),
    Avg { sum: f64, count: i64 },
    /// Welford online moments. `m2` is the sum of squared deviations.
    Welford { count: i64, mean: f64, m2: f64 },
    /// Max-timestamp pick for `Reduction::Last`.
    LastPick { value: f64, ts: i64 },
}

impl Combiner {
    pub fn ingest(&mut self, red: Reduction, v: Option<f64>) -> DapResult<()> {
        self.ingest_timestamped(red, v, i64::MIN)
    }

    /// Variant that accepts a timestamp key for `Reduction::Last`. For other reductions the
    /// timestamp is ignored.
    pub fn ingest_timestamped(
        &mut self,
        red: Reduction,
        v: Option<f64>,
        ts: i64,
    ) -> DapResult<()> {
        self.terminal = Some(red);
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
            (Reduction::StdDev | Reduction::Variance, None) => State::Welford {
                count: 1,
                mean: v,
                m2: 0.0,
            },
            (
                Reduction::StdDev | Reduction::Variance,
                Some(State::Welford { count, mean, m2 }),
            ) => {
                let new_count = count + 1;
                let delta = v - mean;
                let new_mean = mean + delta / new_count as f64;
                let delta2 = v - new_mean;
                State::Welford {
                    count: new_count,
                    mean: new_mean,
                    m2: m2 + delta * delta2,
                }
            }
            (Reduction::Last, None) => State::LastPick { value: v, ts },
            (Reduction::Last, Some(State::LastPick { ts: old_ts, value })) => {
                if ts >= old_ts {
                    State::LastPick { value: v, ts }
                } else {
                    State::LastPick { value, ts: old_ts }
                }
            }
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
        let terminal = self.terminal;
        match self.state {
            None => None,
            Some(State::Sum(v)) | Some(State::Min(v)) | Some(State::Max(v)) => Some(v),
            Some(State::Count(c)) => Some(c as f64),
            Some(State::Avg { sum, count }) if count > 0 => Some(sum / count as f64),
            Some(State::Avg { .. }) => None,
            Some(State::LastPick { value, .. }) => Some(value),
            Some(State::Welford { count, m2, .. }) if count > 0 => {
                let variance = m2 / count as f64;
                match terminal {
                    Some(Reduction::StdDev) => Some(variance.sqrt()),
                    _ => Some(variance),
                }
            }
            Some(State::Welford { .. }) => None,
        }
    }
}

/// Collapse a cell-level table over the (period, org_unit) axes using the DE's aggregation
/// type. Timestamps default to `i64::MIN` which means the `Last` reduction will keep the
/// last-inserted value.
pub fn collapse_cells(
    pair: ReductionPair,
    rows: impl IntoIterator<Item = (String, String, Option<f64>)>,
) -> DapResult<Option<f64>> {
    collapse_cells_ts(
        pair,
        rows.into_iter().map(|(p, o, v)| (p, o, v, i64::MIN)),
    )
}

/// Timestamped version of `collapse_cells`, required for `Reduction::Last`. The timestamp
/// is used to pick the latest value per OU and then (for `LAST`) the latest across OUs.
pub fn collapse_cells_ts(
    pair: ReductionPair,
    rows: impl IntoIterator<Item = (String, String, Option<f64>, i64)>,
) -> DapResult<Option<f64>> {
    if matches!(pair.time, Reduction::None) || matches!(pair.org_unit, Reduction::None) {
        return Err(DapError::invalid_value(
            "aggregation type 'NONE' cannot collapse cells; planner must handle raw cells",
        ));
    }
    // STDDEV / VARIANCE must pool all values across both axes to match the textbook
    // definition. Nesting (Welford over time, then Welford over the resulting means) would
    // produce a variance-of-means which is not what DHIS2 publishes.
    if matches!(pair.time, Reduction::StdDev | Reduction::Variance)
        && pair.time == pair.org_unit
    {
        let mut c = Combiner::default();
        for (_p, _ou, v, ts) in rows {
            c.ingest_timestamped(pair.time, v, ts)?;
        }
        return Ok(c.finish());
    }
    let mut by_ou: ahash::AHashMap<String, (Combiner, i64)> = ahash::AHashMap::default();
    for (_period, ou, v, ts) in rows {
        let entry = by_ou.entry(ou).or_default();
        entry.0.ingest_timestamped(pair.time, v, ts)?;
        if ts > entry.1 {
            entry.1 = ts;
        }
    }
    let mut outer = Combiner::default();
    for (_ou, (c, latest_ts)) in by_ou {
        outer.ingest_timestamped(pair.org_unit, c.finish(), latest_ts)?;
    }
    Ok(outer.finish())
}

#[cfg(test)]
#[allow(clippy::useless_vec)]
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

    fn ts_cells() -> Vec<(String, String, Option<f64>, i64)> {
        // Timestamps grow with period: 202601=1, 202602=2.
        vec![
            ("202601".into(), "ouA".into(), Some(10.0), 1),
            ("202602".into(), "ouA".into(), Some(30.0), 2),
            ("202601".into(), "ouB".into(), Some(20.0), 1),
            ("202602".into(), "ouB".into(), Some(40.0), 2),
        ]
    }

    #[test]
    fn sum_both_axes() {
        let p = ReductionPair::from_aggregation_type(AggregationType::Sum).unwrap();
        assert_eq!(collapse_cells(p, cells()).unwrap(), Some(100.0));
    }

    #[test]
    fn average_sum_org_unit_matches_dhis2_formula() {
        let p =
            ReductionPair::from_aggregation_type(AggregationType::AverageSumOrgUnit).unwrap();
        assert_eq!(collapse_cells(p, cells()).unwrap(), Some(50.0));
    }

    #[test]
    fn average_both_axes() {
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

    #[test]
    fn variance_and_stddev_match_textbook() {
        // Values 2, 4, 4, 4, 5, 5, 7, 9 have population variance 4.0 and stddev 2.0.
        let rows = vec![
            ("202601".into(), "ouA".into(), Some(2.0)),
            ("202602".into(), "ouA".into(), Some(4.0)),
            ("202603".into(), "ouA".into(), Some(4.0)),
            ("202604".into(), "ouA".into(), Some(4.0)),
            ("202605".into(), "ouA".into(), Some(5.0)),
            ("202606".into(), "ouA".into(), Some(5.0)),
            ("202607".into(), "ouA".into(), Some(7.0)),
            ("202608".into(), "ouA".into(), Some(9.0)),
        ];
        let p_var =
            ReductionPair::from_aggregation_type(AggregationType::Variance).unwrap();
        let var = collapse_cells(p_var, rows.clone()).unwrap().unwrap();
        assert!((var - 4.0).abs() < 1e-9, "variance = {var}");

        let p_std =
            ReductionPair::from_aggregation_type(AggregationType::StdDev).unwrap();
        let sd = collapse_cells(p_std, rows).unwrap().unwrap();
        assert!((sd - 2.0).abs() < 1e-9, "stddev = {sd}");
    }

    #[test]
    fn last_picks_highest_timestamp_per_ou_and_overall() {
        // Per-OU LAST: A = 30 (ts=2), B = 40 (ts=2). Across OUs with LAST again, the later
        // tie-break is implementation-defined but the value must be one of {30, 40}.
        let p_last = ReductionPair::from_aggregation_type(AggregationType::Last).unwrap();
        let v = collapse_cells_ts(p_last, ts_cells()).unwrap().unwrap();
        assert!(v == 30.0 || v == 40.0, "LAST returned {v}");
    }

    #[test]
    fn last_in_period_averages_across_ous() {
        // LAST across time per OU = 30, 40. AVG across OUs = 35.
        let p =
            ReductionPair::from_aggregation_type(AggregationType::LastInPeriod).unwrap();
        assert_eq!(collapse_cells_ts(p, ts_cells()).unwrap(), Some(35.0));
    }
}
