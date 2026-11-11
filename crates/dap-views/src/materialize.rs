//! Build `MaterializedView` instances for DHIS2 Indicators and Program Indicators.
//!
//! The materialiser walks a `(org_unit, period)` grid, evaluates the indicator at each
//! cell, and stuffs the results into a `RecordBatch`. The result + manifest form a
//! `MaterializedView` the host registers with the view catalogue.

use ahash::AHashMap;
use dap_core::{Column, DapResult, Fingerprint, LogicalType, Schema, Uid, Value};
use dap_dhis2::{
    evaluate_enrollment_program_indicator, evaluate_event_program_indicator, evaluate_indicator,
    AnalyticsType, EnrollmentContext, EventFact, IndicatorPlan, ProgramIndicatorPlan,
};
use dap_runtime::batch::{RecordBatch, RecordBatchBuilder};

use crate::freshness::FreshnessStamp;
use crate::view::{MaterializedView, ViewManifest};

/// Per-cell aggregate-data lookup. The caller supplies a function that answers the sum of
/// a DE's value for a given `(ou, period)` cell. Null → NULL propagation.
pub type AggregateCellLookup<'a> = dyn FnMut(&Uid, &str, &str, Option<&str>) -> Option<f64> + 'a;

#[derive(Debug, Clone)]
pub struct PiMaterialization {
    pub view: MaterializedView,
    pub rows_computed: u64,
    pub null_cells: u64,
}

fn grid_schema() -> Schema {
    Schema::new(vec![
        Column::new("ou", LogicalType::Uid).required(),
        Column::new("period", LogicalType::Period).required(),
        Column::new("value", LogicalType::F64),
    ])
    .unwrap()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Materialise a DHIS2 (classic) Indicator — `(num, denom, factor)` — over a grid.
pub fn materialize_aggregate_indicator(
    id: impl Into<String>,
    owner: impl Into<String>,
    plan: &IndicatorPlan,
    org_units: &[Uid],
    periods: &[String],
    metadata_revision: Fingerprint,
    mut lookup: Box<AggregateCellLookup<'_>>,
) -> DapResult<PiMaterialization> {
    let schema = grid_schema();
    let mut builder = RecordBatchBuilder::new(schema.clone());
    let mut rows = 0u64;
    let mut nulls = 0u64;
    for ou in org_units {
        for period in periods {
            // Resolver answers #{DE} and #{DE.COC} via the supplied lookup.
            let mut resolver = |de: &str, coc: Option<&str>| lookup(ou, period, de, coc);
            let v = evaluate_indicator(plan, &mut resolver)?;
            builder.append_row(&[
                Value::Str(ou.as_str().to_string()),
                Value::Str(period.clone()),
                match v {
                    Some(n) => Value::F64(n),
                    None => {
                        nulls += 1;
                        Value::Null
                    }
                },
            ])?;
            rows += 1;
        }
    }
    let batch = builder.finish()?;
    let view = MaterializedView::new(
        ViewManifest {
            id: id.into(),
            owner: owner.into(),
            grain: vec!["ou".into(), "period".into()],
            measures: vec!["value".into()],
            consumers: vec![],
            freshness: FreshnessStamp {
                upstream_watermark_ms: now_ms(),
                metadata_revision,
                last_refresh_ms: now_ms(),
                rows,
            },
        },
        batch,
    );
    Ok(PiMaterialization {
        view,
        rows_computed: rows,
        null_cells: nulls,
    })
}

/// Materialise an EVENT-grain Program Indicator over `(ou, period)` cells by grouping the
/// supplied event stream per cell.
pub fn materialize_event_program_indicator(
    id: impl Into<String>,
    owner: impl Into<String>,
    plan: &ProgramIndicatorPlan,
    events: &[EventFact],
    org_units: &[Uid],
    periods: &[(String, i64, i64)], // (code, start_ms, end_ms)
    metadata_revision: Fingerprint,
) -> DapResult<PiMaterialization> {
    if plan.analytics_type != AnalyticsType::Event {
        return Err(dap_core::DapError::invalid_value(
            "materialize_event_program_indicator requires AnalyticsType::Event",
        ));
    }

    // Pre-index events by (ou, period code). Using a map keeps per-cell evaluation O(events
    // in cell) rather than O(all events).
    let mut by_cell: AHashMap<(Uid, String), Vec<&EventFact>> = AHashMap::default();
    for event in events {
        if event.program != plan.program {
            continue;
        }
        for (code, start, end) in periods {
            if event.event_date_ms >= *start && event.event_date_ms < *end {
                by_cell
                    .entry((event.org_unit.clone(), code.clone()))
                    .or_default()
                    .push(event);
            }
        }
    }

    let schema = grid_schema();
    let mut builder = RecordBatchBuilder::new(schema.clone());
    let mut rows = 0u64;
    let mut nulls = 0u64;
    for ou in org_units {
        for (code, _, _) in periods {
            let bucket: &[&EventFact] = by_cell
                .get(&(ou.clone(), code.clone()))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            let r =
                evaluate_event_program_indicator(plan, bucket.iter().copied())?;
            builder.append_row(&[
                Value::Str(ou.as_str().to_string()),
                Value::Str(code.clone()),
                match r.value {
                    Some(n) => Value::F64(n),
                    None => {
                        nulls += 1;
                        Value::Null
                    }
                },
            ])?;
            rows += 1;
        }
    }
    let batch = builder.finish()?;
    Ok(PiMaterialization {
        view: MaterializedView::new(
            ViewManifest {
                id: id.into(),
                owner: owner.into(),
                grain: vec!["ou".into(), "period".into()],
                measures: vec!["value".into()],
                consumers: vec![],
                freshness: FreshnessStamp {
                    upstream_watermark_ms: now_ms(),
                    metadata_revision,
                    last_refresh_ms: now_ms(),
                    rows,
                },
            },
            batch,
        ),
        rows_computed: rows,
        null_cells: nulls,
    })
}

/// Materialise an ENROLLMENT-grain Program Indicator. `contexts_per_cell` yields the
/// enrollment contexts that belong to a given `(ou, period)` cell. The caller owns the
/// grouping so the materialiser stays free of event-date-to-period math for tracker
/// programs (enrollment date, incident date, completion date all compete).
pub fn materialize_enrollment_program_indicator<'a, F>(
    id: impl Into<String>,
    owner: impl Into<String>,
    plan: &ProgramIndicatorPlan,
    org_units: &[Uid],
    periods: &[String],
    metadata_revision: Fingerprint,
    mut contexts_per_cell: F,
) -> DapResult<PiMaterialization>
where
    F: FnMut(&Uid, &str) -> Vec<EnrollmentContext<'a>>,
{
    if plan.analytics_type != AnalyticsType::Enrollment {
        return Err(dap_core::DapError::invalid_value(
            "materialize_enrollment_program_indicator requires AnalyticsType::Enrollment",
        ));
    }
    let schema = grid_schema();
    let mut builder = RecordBatchBuilder::new(schema.clone());
    let mut rows = 0u64;
    let mut nulls = 0u64;
    for ou in org_units {
        for period in periods {
            let ctx_vec = contexts_per_cell(ou, period);
            let r = evaluate_enrollment_program_indicator(plan, ctx_vec)?;
            builder.append_row(&[
                Value::Str(ou.as_str().to_string()),
                Value::Str(period.clone()),
                match r.value {
                    Some(n) => Value::F64(n),
                    None => {
                        nulls += 1;
                        Value::Null
                    }
                },
            ])?;
            rows += 1;
        }
    }
    let batch = builder.finish()?;
    Ok(PiMaterialization {
        view: MaterializedView::new(
            ViewManifest {
                id: id.into(),
                owner: owner.into(),
                grain: vec!["ou".into(), "period".into()],
                measures: vec!["value".into()],
                consumers: vec![],
                freshness: FreshnessStamp {
                    upstream_watermark_ms: now_ms(),
                    metadata_revision,
                    last_refresh_ms: now_ms(),
                    rows,
                },
            },
            batch,
        ),
        rows_computed: rows,
        null_cells: nulls,
    })
}

/// Silence the unused `RecordBatch` import linter when the `RecordBatch` type is only used
/// via trait methods.
#[allow(dead_code)]
fn _touch(_b: RecordBatch) {}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_dhis2::compile_indicator;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    #[test]
    fn materialise_classic_indicator_grid() {
        let plan =
            compile_indicator("#{deANC00001}", "#{dePOP00001}", 100).unwrap();
        let ous = vec![uid("ouAAAAAAAAA"), uid("ouBBBBBBBBB")];
        let periods = vec!["202601".to_string(), "202602".to_string()];
        let mut data: AHashMap<(String, String, String), f64> = AHashMap::default();
        data.insert(("ouAAAAAAAAA".into(), "202601".into(), "deANC00001".into()), 80.0);
        data.insert(("ouAAAAAAAAA".into(), "202601".into(), "dePOP00001".into()), 100.0);
        data.insert(("ouAAAAAAAAA".into(), "202602".into(), "deANC00001".into()), 90.0);
        data.insert(("ouAAAAAAAAA".into(), "202602".into(), "dePOP00001".into()), 100.0);
        // ouB missing deANC00001 for 202601 — expect NULL.
        data.insert(("ouBBBBBBBBB".into(), "202601".into(), "dePOP00001".into()), 100.0);
        data.insert(("ouBBBBBBBBB".into(), "202602".into(), "deANC00001".into()), 75.0);
        data.insert(("ouBBBBBBBBB".into(), "202602".into(), "dePOP00001".into()), 100.0);
        let lookup = Box::new(move |ou: &Uid, period: &str, de: &str, _coc: Option<&str>| {
            data.get(&(ou.as_str().to_string(), period.to_string(), de.to_string()))
                .copied()
        });
        let m = materialize_aggregate_indicator(
            "mv_anc_coverage",
            "mother-ncd-team",
            &plan,
            &ous,
            &periods,
            Fingerprint::of(b"rev-1"),
            lookup,
        )
        .unwrap();
        assert_eq!(m.rows_computed, 4);
        assert_eq!(m.null_cells, 1);
        assert_eq!(m.view.body().row_count(), 4);
        assert_eq!(m.view.manifest.grain, vec!["ou".to_string(), "period".to_string()]);
    }
}
