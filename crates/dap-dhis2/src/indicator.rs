//! DHIS2 Indicator compiler.
//!
//! An Indicator is a metadata object with three fields that interest the engine:
//! `numerator` (expression), `denominator` (expression) and `factor` (integer multiplier
//! applied to the ratio). Both expressions use the same grammar as Program Indicators.
//!
//! DHIS2 semantics we implement:
//!
//! 1. `numerator` and `denominator` compile with [`crate::expression`]. Unsupported tokens
//!    (functions, `C{}`, `V{}`, `[days]`, reporting-rate variants) reject at compile time.
//! 2. Null propagation: if any referenced data element has no value for the evaluation
//!    row/group, the indicator yields `None`.
//! 3. Divide-by-zero: when the denominator evaluates to `0`, the indicator yields `None`
//!    (DHIS2 convention, matches the reference implementation for aggregate indicators).
//! 4. Factor: multiplied against the ratio as a pure number — the engine does not round or
//!    truncate. Callers may format the result for presentation.
//! 5. The lookup is done via an `IndicatorResolver`, a trait-object closure the caller
//!    provides. Typical implementations map the (DE, COC) pair to a pre-aggregated SUM
//!    result row keyed by (OU, period, DE, COC).

use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

use crate::expression::{compile, evaluate, validate_functions, ExpressionAst, RefKind};

/// Trait describing how to resolve a `#{DE}` or `#{DE.COC}` reference into a numeric value
/// for the evaluation context (typically a single OU/period/aocc group).
pub trait IndicatorResolver {
    fn value(&mut self, de: &str, coc: Option<&str>) -> Option<f64>;
}

impl<F> IndicatorResolver for F
where
    F: FnMut(&str, Option<&str>) -> Option<f64>,
{
    fn value(&mut self, de: &str, coc: Option<&str>) -> Option<f64> {
        self(de, coc)
    }
}

/// Compiled indicator: typed numerator + denominator ASTs plus the integer factor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IndicatorPlan {
    pub numerator: ExpressionAst,
    pub denominator: ExpressionAst,
    pub factor: i64,
}

impl IndicatorPlan {
    /// References collected from both expressions. Useful to compute the set of data
    /// elements an indicator depends on (used by lineage + materialized-view invalidation).
    pub fn referenced_data_elements(&self) -> Vec<(String, Option<String>)> {
        let mut out = Vec::new();
        collect_refs(&self.numerator, &mut out);
        collect_refs(&self.denominator, &mut out);
        out.sort();
        out.dedup();
        out
    }
}

/// Compile a DHIS2 Indicator into an `IndicatorPlan`. Fails closed on any token the engine
/// does not implement (function calls, constants, variables, reporting-rate suffixes, etc.).
pub fn compile_indicator(
    numerator_src: &str,
    denominator_src: &str,
    factor: i64,
) -> DapResult<IndicatorPlan> {
    let numerator = compile(numerator_src)
        .and_then(|ast| {
            validate_functions(&ast)?;
            Ok(ast)
        })
        .map_err(|e| DapError::invalid_value(format!("indicator numerator: {e}")))?;
    let denominator = compile(denominator_src)
        .and_then(|ast| {
            validate_functions(&ast)?;
            Ok(ast)
        })
        .map_err(|e| DapError::invalid_value(format!("indicator denominator: {e}")))?;
    Ok(IndicatorPlan {
        numerator,
        denominator,
        factor,
    })
}

/// Evaluate a compiled indicator. Returns `None` when any referenced data element has no
/// value, or when the denominator is zero.
pub fn evaluate_indicator(
    plan: &IndicatorPlan,
    resolver: &mut dyn IndicatorResolver,
) -> DapResult<Option<f64>> {
    let mut adapter = |de: &str, coc: Option<&str>| resolver.value(de, coc);
    let num = evaluate(&plan.numerator, &mut adapter)?;
    let den = evaluate(&plan.denominator, &mut adapter)?;
    match (num, den) {
        (Some(n), Some(d)) if d != 0.0 => Ok(Some((n / d) * plan.factor as f64)),
        _ => Ok(None),
    }
}

fn collect_refs(ast: &ExpressionAst, out: &mut Vec<(String, Option<String>)>) {
    match ast {
        ExpressionAst::Null
        | ExpressionAst::Number(_)
        | ExpressionAst::Bool(_)
        | ExpressionAst::String(_) => {}
        ExpressionAst::Ref(r) => {
            if let RefKind::DataElement { de, coc, .. } = r {
                out.push((de.clone(), coc.clone()));
            }
        }
        ExpressionAst::Unary { inner, .. } => collect_refs(inner, out),
        ExpressionAst::Binary { lhs, rhs, .. } => {
            collect_refs(lhs, out);
            collect_refs(rhs, out);
        }
        ExpressionAst::Call { args, .. } => {
            for a in args {
                collect_refs(a, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ahash::AHashMap;

    fn run(num: &str, den: &str, factor: i64, values: &[(&str, f64)]) -> Option<f64> {
        let plan = compile_indicator(num, den, factor).unwrap();
        let map: AHashMap<String, f64> =
            values.iter().map(|(k, v)| ((*k).to_string(), *v)).collect();
        let mut resolver = |de: &str, _coc: Option<&str>| map.get(de).copied();
        evaluate_indicator(&plan, &mut resolver).unwrap()
    }

    /// Coverage-style indicator: ANC1 coverage = (ANC1 attendances / Expected pregnancies) * 100.
    #[test]
    fn coverage_indicator_computes_percentage() {
        let got = run(
            "#{deANCATTEND1}",
            "#{deEXPECTEDPR}",
            100,
            &[("deANCATTEND1", 80.0), ("deEXPECTEDPR", 100.0)],
        );
        assert_eq!(got, Some(80.0));
    }

    /// Per-100 000 rate: incidence = (cases / population) * 100000.
    #[test]
    fn rate_per_100k() {
        let got = run(
            "#{deCASESXXXX}",
            "#{dePOPULATION}",
            100_000,
            &[("deCASESXXXX", 12.0), ("dePOPULATION", 5_000.0)],
        );
        // 12 / 5000 * 100000 = 240. FP rounding keeps us within one ULP of 240.
        let v = got.unwrap();
        assert!((v - 240.0).abs() < 1e-9, "got {v}");
    }

    /// Composite numerator: (a + b) / (c + d) * factor.
    #[test]
    fn composite_expression_ratio() {
        let got = run(
            "#{deA000000000} + #{deB000000000}",
            "#{deC000000000} + #{deD000000000}",
            100,
            &[
                ("deA000000000", 30.0),
                ("deB000000000", 20.0),
                ("deC000000000", 40.0),
                ("deD000000000", 60.0),
            ],
        );
        assert_eq!(got, Some(50.0));
    }

    /// Divide-by-zero yields NULL per DHIS2 convention.
    #[test]
    fn zero_denominator_is_null() {
        let got = run(
            "#{deAAAAAAAAA}",
            "#{deBBBBBBBBB}",
            100,
            &[("deAAAAAAAAA", 5.0), ("deBBBBBBBBB", 0.0)],
        );
        assert_eq!(got, None);
    }

    /// Missing data element in the numerator propagates to NULL.
    #[test]
    fn missing_numerator_de_is_null() {
        let got = run(
            "#{deAAAAAAAAA}",
            "#{deBBBBBBBBB}",
            100,
            &[("deBBBBBBBBB", 50.0)],
        );
        assert_eq!(got, None);
    }

    /// Factor is applied exactly — no rounding by the engine.
    #[test]
    fn factor_preserves_precision() {
        let plan = compile_indicator("#{deA000000000}", "#{deB000000000}", 1).unwrap();
        let mut resolver = |de: &str, _coc: Option<&str>| {
            match de {
                "deA000000000" => Some(1.0),
                "deB000000000" => Some(3.0),
                _ => None,
            }
        };
        let v = evaluate_indicator(&plan, &mut resolver).unwrap().unwrap();
        assert!((v - 1.0 / 3.0).abs() < 1e-12);
    }

    /// Unsupported constructs in the numerator surface as a compile error, not a silent
    /// null. Spec §6.1 explicitly requires this.
    #[test]
    fn unsupported_function_in_numerator_rejected() {
        let err = compile_indicator("sum(#{deA000000000})", "#{deB000000000}", 100)
            .expect_err("sum() must be rejected");
        let msg = err.to_string();
        assert!(msg.contains("numerator"), "error must flag which side: {msg}");
        assert!(msg.contains("function") || msg.contains("not in the v1"), "got: {msg}");
    }

    #[test]
    fn unsupported_in_denominator_rejected() {
        let err = compile_indicator("#{deA000000000}", "avg(#{deB000000000})", 100)
            .expect_err("avg() must be rejected");
        assert!(err.to_string().contains("denominator"));
    }

    #[test]
    fn referenced_data_elements_enumerated() {
        let plan = compile_indicator(
            "#{deA000000000} + #{deB000000000}",
            "#{deA000000000}",
            100,
        )
        .unwrap();
        let refs = plan.referenced_data_elements();
        // deA appears in both numerator and denominator -> dedup to one entry.
        assert_eq!(refs.len(), 2);
        assert!(refs.contains(&("deA000000000".into(), None)));
        assert!(refs.contains(&("deB000000000".into(), None)));
    }

    /// COC-scoped references propagate end-to-end.
    #[test]
    fn de_coc_reference_resolved() {
        let plan = compile_indicator(
            "#{deANCATTEND1.cocFEMALEADULT}",
            "#{deEXPECTEDPR}",
            100,
        )
        .unwrap();
        let mut resolver = |de: &str, coc: Option<&str>| match (de, coc) {
            ("deANCATTEND1", Some("cocFEMALEADULT")) => Some(80.0),
            ("deEXPECTEDPR", None) => Some(100.0),
            _ => None,
        };
        assert_eq!(evaluate_indicator(&plan, &mut resolver).unwrap(), Some(80.0));
    }
}
