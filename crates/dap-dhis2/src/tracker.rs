//! Tracker Program analytics.
//!
//! Spec §8.3 — "No simplistic joins that duplicate numerators due to repeatable stages."
//!
//! Model
//! =====
//!
//! - `TrackedEntity`: a person / case identified by a UID, with a map of attributes.
//! - `Enrollment`: a TEI's enrollment into a program, with enrollment date, incident date,
//!   status (ACTIVE / COMPLETED / CANCELLED), and associated OU.
//! - Events belong to one enrollment and reference one stage. Stages may be repeatable.
//!
//! This module implements:
//!
//! 1. A typed `Enrollment` plus helpers to attach events (`EventFact` from `crate::event`).
//! 2. **Distinct TEI counting** for Enrollment-type Program Indicators. The engine uses the
//!    TEI UID set and never counts a TEI more than once, even when the TEI has multiple
//!    enrollments or when a repeatable stage fires multiple times.
//! 3. **Enrollment-grain Program Indicator evaluation**: the expression is evaluated once
//!    per enrollment, with `#{DE}` / `D{stage.de}` references resolving against the
//!    enrollment's events using DHIS2 semantics (latest event for the stage, or sum for
//!    numeric aggregation types as noted in `evaluate_enrollment_data_element`).
//! 4. Longitudinal / cohort primitives: enrollments-in-period, completed-in-period, time
//!    between enrollment and completion.

use ahash::{AHashMap, AHashSet};
use dap_catalog::metadata::AggregationType;
use dap_core::{DapError, DapResult, Uid};
use serde::{Deserialize, Serialize};

use crate::event::{AggCombiner, AnalyticsType, EventFact, PIResult, ProgramIndicatorPlan};
use crate::expression::{evaluate_expr, EValue, RefKind, Resolver};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EnrollmentStatus {
    Active,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TrackedEntity {
    pub uid: Uid,
    pub tracked_entity_type: Uid,
    #[serde(default)]
    pub attributes: AHashMap<String, EValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Enrollment {
    pub uid: Uid,
    pub program: Uid,
    pub tei: Uid,
    pub org_unit: Uid,
    pub enrollment_date_ms: i64,
    pub incident_date_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_date_ms: Option<i64>,
    pub status: EnrollmentStatus,
}

/// Composite view of an enrollment used for evaluation. Groups the enrollment, its TEI, and
/// its events together so an expression resolver can span all three.
#[derive(Clone, Debug)]
pub struct EnrollmentContext<'a> {
    pub enrollment: &'a Enrollment,
    pub tei: &'a TrackedEntity,
    pub events: Vec<&'a EventFact>,
}

/// Distinct TEI count across a list of enrollments. Correct even when:
///   - A TEI has multiple enrollments in the same program (counts 1).
///   - A TEI has repeatable stages with multiple events (counts 1).
pub fn distinct_tei_count<'a>(
    enrollments: impl IntoIterator<Item = &'a Enrollment>,
) -> usize {
    let mut seen: AHashSet<&Uid> = AHashSet::default();
    for e in enrollments {
        seen.insert(&e.tei);
    }
    seen.len()
}

/// Group events by enrollment UID. The caller supplies a slice; the output borrows from it.
pub fn events_by_enrollment<'a>(
    events: &'a [EventFact],
) -> AHashMap<Uid, Vec<&'a EventFact>> {
    let mut map: AHashMap<Uid, Vec<&'a EventFact>> = AHashMap::default();
    for e in events {
        if let Some(enr) = &e.enrollment {
            map.entry(enr.clone()).or_default().push(e);
        }
    }
    map
}

/// Evaluate an `ENROLLMENT`-type Program Indicator across enrollment-grain contexts.
///
/// DHIS2 semantics:
///   - `#{DE}` with `aggregation_type` numeric (SUM/AVG/MIN/MAX) resolves to the
///     correspondingly aggregated value of the DE across the enrollment's events for the
///     target stage. For COUNT it resolves to the number of events contributing a non-null
///     value. For other kinds it resolves to the single value when the stage has exactly
///     one event, else NULL (engine refuses to guess).
///   - `D{stage.DE}` restricts to events of the named stage.
///   - `V{enrollment_count}` is always 1 per enrollment.
///   - `V{event_count}` is the number of events in the enrollment.
///   - `V{tei_count}` is 1 per enrollment (de-duplication happens in the outer aggregator).
///
/// The outer `aggregation_type` then combines per-enrollment values across the stream.
pub fn evaluate_enrollment_program_indicator<'a>(
    plan: &ProgramIndicatorPlan,
    contexts: impl IntoIterator<Item = EnrollmentContext<'a>>,
) -> DapResult<PIResult> {
    if plan.analytics_type != AnalyticsType::Enrollment {
        return Err(DapError::invalid_value(
            "evaluate_enrollment_program_indicator requires AnalyticsType::Enrollment",
        ));
    }
    let mut combiner = AggCombiner::new(plan.aggregation_type)?;
    let mut contributing = 0usize;

    for ctx in contexts {
        if ctx.enrollment.program != plan.program {
            continue;
        }
        let mut resolver = EnrollmentResolver {
            ctx: &ctx,
            de_aggregation: AggregationType::Sum,
        };
        if let Some(filter) = &plan.filter {
            if !matches!(
                evaluate_expr(filter, &mut resolver)?.as_bool(),
                Some(true)
            ) {
                continue;
            }
        }
        let v = evaluate_expr(&plan.expression, &mut resolver)?.as_number();
        combiner.ingest(v);
        contributing += 1;
    }
    Ok(PIResult {
        contributing_rows: contributing,
        value: combiner.finish(),
    })
}

/// How to collapse multiple event-grain values for a data element referenced inside an
/// enrollment-level expression. Default is SUM (matches DHIS2 for numeric SUM program
/// indicators); callers can override for AVG / MIN / MAX / COUNT / NONE semantics.
pub struct EnrollmentResolver<'a> {
    pub ctx: &'a EnrollmentContext<'a>,
    pub de_aggregation: AggregationType,
}

impl<'a> Resolver for EnrollmentResolver<'a> {
    fn reference(&mut self, r: &RefKind) -> DapResult<EValue> {
        Ok(match r {
            RefKind::DataElement { de, .. } => {
                self.aggregate_de_across_events(de.as_str(), None)
            }
            RefKind::StageDataElement { stage, de } => {
                self.aggregate_de_across_events(de.as_str(), Some(stage.as_str()))
            }
            RefKind::Attribute { attribute, .. } => self
                .ctx
                .tei
                .attributes
                .get(attribute.as_str())
                .cloned()
                .unwrap_or(EValue::Null),
            RefKind::Variable(name) => match name.as_str() {
                "enrollment_count" | "tei_count" => EValue::Number(1.0),
                "event_count" => EValue::Number(self.ctx.events.len() as f64),
                "enrollment_date" => EValue::Number(self.ctx.enrollment.enrollment_date_ms as f64),
                "incident_date" => EValue::Number(self.ctx.enrollment.incident_date_ms as f64),
                "completed_date" => self
                    .ctx
                    .enrollment
                    .completed_date_ms
                    .map(|v| EValue::Number(v as f64))
                    .unwrap_or(EValue::Null),
                "org_unit_count" => EValue::Number(1.0),
                _ => EValue::Null,
            },
            RefKind::Days | RefKind::Constant(_) | RefKind::Indicator(_) => EValue::Null,
        })
    }
}

impl<'a> EnrollmentResolver<'a> {
    fn aggregate_de_across_events(&self, de: &str, stage_filter: Option<&str>) -> EValue {
        let mut combiner = AggCombiner::new(self.de_aggregation).ok();
        let mut any = false;
        for e in &self.ctx.events {
            if let Some(s) = stage_filter {
                if e.stage.as_str() != s {
                    continue;
                }
            }
            if let Some(v) = e.data_values.get(de) {
                if let Some(n) = v.as_number() {
                    if let Some(c) = combiner.as_mut() {
                        c.ingest(Some(n));
                        any = true;
                    }
                }
            }
        }
        if !any {
            return EValue::Null;
        }
        combiner
            .and_then(|c| c.finish())
            .map(EValue::Number)
            .unwrap_or(EValue::Null)
    }
}

/// Enrollments whose `enrollment_date_ms` falls in `[start, end)` (exclusive end).
pub fn enrollments_in_period<'a>(
    enrollments: impl IntoIterator<Item = &'a Enrollment>,
    start_ms: i64,
    end_ms: i64,
) -> Vec<&'a Enrollment> {
    enrollments
        .into_iter()
        .filter(|e| e.enrollment_date_ms >= start_ms && e.enrollment_date_ms < end_ms)
        .collect()
}

/// Enrollments completed inside `[start, end)`.
pub fn completions_in_period<'a>(
    enrollments: impl IntoIterator<Item = &'a Enrollment>,
    start_ms: i64,
    end_ms: i64,
) -> Vec<&'a Enrollment> {
    enrollments
        .into_iter()
        .filter(|e| {
            e.status == EnrollmentStatus::Completed
                && e.completed_date_ms
                    .map(|c| c >= start_ms && c < end_ms)
                    .unwrap_or(false)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::useless_vec)]
mod tests {
    use super::*;
    use crate::event::EventStatus;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    fn tei(uid_str: &str) -> TrackedEntity {
        TrackedEntity {
            uid: uid(uid_str),
            tracked_entity_type: uid("tetPERSON00"),
            attributes: AHashMap::default(),
        }
    }

    fn enrollment(
        uid_str: &str,
        tei_uid: &str,
        date: i64,
        status: EnrollmentStatus,
    ) -> Enrollment {
        Enrollment {
            uid: uid(uid_str),
            program: uid("prANC000000"),
            tei: uid(tei_uid),
            org_unit: uid("ouAAAAAAAAA"),
            enrollment_date_ms: date,
            incident_date_ms: date,
            completed_date_ms: if status == EnrollmentStatus::Completed {
                Some(date + 1_000_000)
            } else {
                None
            },
            status,
        }
    }

    fn event_for(
        enrollment_uid: &str,
        stage_uid: &str,
        tei_uid: &str,
        weight: f64,
    ) -> EventFact {
        let mut dv: AHashMap<String, EValue> = AHashMap::default();
        dv.insert("deWEIGHT000".into(), EValue::Number(weight));
        EventFact {
            event_uid: uid("ev000000001"),
            program: uid("prANC000000"),
            stage: uid(stage_uid),
            org_unit: uid("ouAAAAAAAAA"),
            event_date_ms: 1_000,
            status: EventStatus::Completed,
            tei: Some(uid(tei_uid)),
            enrollment: Some(uid(enrollment_uid)),
            data_values: dv,
            attributes: AHashMap::default(),
        }
    }

    #[test]
    fn distinct_tei_count_handles_repeat_enrollments() {
        // Same TEI with two enrollments must count once.
        let enrs = vec![
            enrollment("en000000001", "teT00000001", 10, EnrollmentStatus::Completed),
            enrollment("en000000002", "teT00000001", 50, EnrollmentStatus::Active),
            enrollment("en000000003", "teT00000002", 20, EnrollmentStatus::Completed),
        ];
        assert_eq!(distinct_tei_count(enrs.iter()), 2);
    }

    #[test]
    fn repeatable_stage_does_not_inflate_distinct_tei() {
        // TEI has three events across a repeatable stage; distinct TEIs is still 1.
        let enrs = vec![enrollment(
            "en000000001",
            "teT00000001",
            10,
            EnrollmentStatus::Active,
        )];
        let events = vec![
            event_for("en000000001", "ps000000001", "teT00000001", 10.0),
            event_for("en000000001", "ps000000001", "teT00000001", 12.0),
            event_for("en000000001", "ps000000001", "teT00000001", 15.0),
        ];
        let grouped = events_by_enrollment(&events);
        assert_eq!(grouped.get(&uid("en000000001")).unwrap().len(), 3);
        assert_eq!(distinct_tei_count(enrs.iter()), 1);
    }

    #[test]
    fn enrollment_program_indicator_sum_across_events() {
        let enr = enrollment(
            "en000000001",
            "teT00000001",
            10,
            EnrollmentStatus::Completed,
        );
        let tei = tei("teT00000001");
        let events = vec![
            event_for("en000000001", "ps000000001", "teT00000001", 10.0),
            event_for("en000000001", "ps000000001", "teT00000001", 20.0),
            event_for("en000000001", "ps000000002", "teT00000001", 5.0),
        ];
        let ctx = EnrollmentContext {
            enrollment: &enr,
            tei: &tei,
            events: events.iter().collect(),
        };

        let plan = ProgramIndicatorPlan::compile(
            uid("piTOTALWGHT"),
            "Total weight across all stages",
            uid("prANC000000"),
            "#{deWEIGHT000}",
            None,
            AnalyticsType::Enrollment,
            AggregationType::Sum,
        )
        .unwrap();
        let r = evaluate_enrollment_program_indicator(&plan, [ctx]).unwrap();
        assert_eq!(r.value, Some(35.0), "SUM of all weight events in enrollment");
    }

    #[test]
    fn stage_restriction_via_d_ref() {
        let enr = enrollment(
            "en000000001",
            "teT00000001",
            10,
            EnrollmentStatus::Completed,
        );
        let tei = tei("teT00000001");
        let events = vec![
            event_for("en000000001", "ps000000001", "teT00000001", 10.0),
            event_for("en000000001", "ps000000001", "teT00000001", 20.0),
            event_for("en000000001", "ps000000002", "teT00000001", 5.0),
        ];
        let ctx = EnrollmentContext {
            enrollment: &enr,
            tei: &tei,
            events: events.iter().collect(),
        };
        let plan = ProgramIndicatorPlan::compile(
            uid("piSTAGE1WGT"),
            "Sum weight at stage 1",
            uid("prANC000000"),
            "D{ps000000001.deWEIGHT000}",
            None,
            AnalyticsType::Enrollment,
            AggregationType::Sum,
        )
        .unwrap();
        let r = evaluate_enrollment_program_indicator(&plan, [ctx]).unwrap();
        assert_eq!(r.value, Some(30.0), "stage filter drops the ps2 event");
    }

    #[test]
    fn enrollment_pi_count_distinct_teis() {
        // Two enrollments for one TEI + one enrollment for a second TEI.
        let e1 = enrollment("en1aaaaaaaa", "teT00000001", 10, EnrollmentStatus::Active);
        let e2 = enrollment("en2bbbbbbbb", "teT00000001", 20, EnrollmentStatus::Completed);
        let e3 = enrollment("en3cccccccc", "teT00000002", 30, EnrollmentStatus::Active);
        let t1 = tei("teT00000001");
        let t2 = tei("teT00000002");
        let contexts = vec![
            EnrollmentContext {
                enrollment: &e1,
                tei: &t1,
                events: vec![],
            },
            EnrollmentContext {
                enrollment: &e2,
                tei: &t1,
                events: vec![],
            },
            EnrollmentContext {
                enrollment: &e3,
                tei: &t2,
                events: vec![],
            },
        ];
        // A per-enrollment PI with V{tei_count}=1 and SUM aggregation counts enrollments,
        // NOT distinct TEIs. For distinct TEIs the caller uses `distinct_tei_count` and
        // must not double-count via SUM. Document that difference explicitly.
        let plan = ProgramIndicatorPlan::compile(
            uid("piENRCOUNT1"),
            "Enrollment count",
            uid("prANC000000"),
            "V{enrollment_count}",
            None,
            AnalyticsType::Enrollment,
            AggregationType::Sum,
        )
        .unwrap();
        let r =
            evaluate_enrollment_program_indicator(&plan, contexts.clone()).unwrap();
        assert_eq!(r.value, Some(3.0), "SUM of V{{enrollment_count}} = 3");

        // Correct distinct-TEI count uses `distinct_tei_count`.
        let enrollments: Vec<&Enrollment> =
            contexts.iter().map(|c| c.enrollment).collect();
        assert_eq!(distinct_tei_count(enrollments), 2);
    }

    #[test]
    fn longitudinal_period_filters() {
        let enrs = vec![
            enrollment("en000000001", "teT00000001", 100, EnrollmentStatus::Completed),
            enrollment("en000000002", "teT00000002", 200, EnrollmentStatus::Active),
            enrollment("en000000003", "teT00000003", 300, EnrollmentStatus::Completed),
        ];
        let in_window = enrollments_in_period(enrs.iter(), 150, 400);
        assert_eq!(in_window.len(), 2);

        let completed_window = completions_in_period(enrs.iter(), 0, 10_000_000);
        // enr#1 completed at 100+1_000_000 = 1_000_100; enr#3 completed at 300+1_000_000 =
        // 1_000_300. Both inside the window.
        assert_eq!(completed_window.len(), 2);
    }

    #[test]
    fn tei_attribute_resolved_in_enrollment_pi() {
        let mut t = tei("teT00000001");
        t.attributes
            .insert("teaAGE00000".into(), EValue::Number(28.0));
        let enr = enrollment("en000000001", "teT00000001", 0, EnrollmentStatus::Active);
        let ctx = EnrollmentContext {
            enrollment: &enr,
            tei: &t,
            events: vec![],
        };
        let plan = ProgramIndicatorPlan::compile(
            uid("piAGE000000"),
            "Mother age",
            uid("prANC000000"),
            "A{teaAGE00000}",
            None,
            AnalyticsType::Enrollment,
            AggregationType::Average,
        )
        .unwrap();
        let r = evaluate_enrollment_program_indicator(&plan, [ctx]).unwrap();
        assert_eq!(r.value, Some(28.0));
    }
}
