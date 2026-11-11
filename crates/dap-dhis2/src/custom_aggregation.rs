//! Custom Program Indicator aggregation.
//!
//! DHIS2 allows a Program Indicator to declare `aggregationType = CUSTOM` with an auxiliary
//! `aggregationExpression` field that the engine evaluates over the per-grain stream of
//! numeric results. DAP accepts a reduced-shape expression: the identifier
//! `V{value}` stands for "the current element of the stream", and the usual operators
//! plus the DHIS2-safe function set are available. The expression is evaluated
//! incrementally so the engine never materialises the full stream.
//!
//! Supported sub-set of DHIS2 custom expressions:
//!
//! - `avg(V{value})`
//! - `sum(V{value})`
//! - `count(V{value})`
//! - `min(V{value})` / `max(V{value})`
//! - `stddev(V{value})` / `variance(V{value})`
//! - `percentile(V{value}, p)` with `p` in \[0, 1\].
//!
//! Plain arithmetic expressions with `avg` / `sum` / etc. as sub-terms are accepted:
//! `100 * sum(V{value}) / count(V{value})` evaluates to the mean times 100. Anything else
//! (including user-defined aggregates we don't recognise) rejects at compile time.

use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

use crate::expression::{
    compile, evaluate_expr, validate_functions, EValue, ExpressionAst, RefKind, Resolver,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CustomAggregationPlan {
    /// Original aggregation expression, kept for provenance.
    pub source: String,
    /// Compiled AST. `V{value}` references are rewritten by `evaluate_custom_aggregation`
    /// into sliding scalars.
    pub ast: ExpressionAst,
    /// The set of named aggregators the expression references, enumerated at compile time.
    pub aggregators: Vec<AggregatorKind>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AggregatorKind {
    Sum,
    Avg,
    Count,
    Min,
    Max,
    StdDev,
    Variance,
    /// Percentile with the probability captured at compile time (0 ≤ p ≤ 1).
    Percentile(u32),
}

impl AggregatorKind {
    fn parse(name: &str, args: &[ExpressionAst]) -> DapResult<Self> {
        match (name, args) {
            ("sum", [_]) => Ok(Self::Sum),
            ("avg" | "average", [_]) => Ok(Self::Avg),
            ("count", [_]) => Ok(Self::Count),
            ("min", [_]) => Ok(Self::Min),
            ("max", [_]) => Ok(Self::Max),
            ("stddev", [_]) => Ok(Self::StdDev),
            ("variance", [_]) => Ok(Self::Variance),
            ("percentile", [_, p]) => {
                let pv = const_eval_numeric(p)?;
                if !(0.0..=1.0).contains(&pv) {
                    return Err(DapError::invalid_value(format!(
                        "percentile p must be in [0,1], got {pv}"
                    )));
                }
                Ok(Self::Percentile((pv * 1_000_000.0).round() as u32))
            }
            (other, _) => Err(DapError::UnsupportedFeature(format!(
                "custom aggregation function '{other}' is not implemented"
            ))),
        }
    }

    fn init_state(&self) -> AggState {
        match self {
            Self::Sum => AggState::Sum { sum: 0.0, count: 0 },
            Self::Count => AggState::Count(0),
            Self::Avg => AggState::Avg { sum: 0.0, count: 0 },
            Self::Min => AggState::Min(f64::INFINITY),
            Self::Max => AggState::Max(f64::NEG_INFINITY),
            Self::StdDev | Self::Variance => AggState::Welford {
                count: 0,
                mean: 0.0,
                m2: 0.0,
            },
            Self::Percentile(_) => AggState::Collect(Vec::new()),
        }
    }
}

fn const_eval_numeric(ast: &ExpressionAst) -> DapResult<f64> {
    match ast {
        ExpressionAst::Number(n) => Ok(*n),
        ExpressionAst::Unary {
            op: crate::expression::UnaryOp::Neg,
            inner,
        } => Ok(-const_eval_numeric(inner)?),
        other => Err(DapError::invalid_value(format!(
            "expected a numeric literal, got {other:?}"
        ))),
    }
}

#[derive(Clone, Debug)]
enum AggState {
    Sum { sum: f64, count: i64 },
    Count(i64),
    Avg { sum: f64, count: i64 },
    Min(f64),
    Max(f64),
    Welford { count: i64, mean: f64, m2: f64 },
    Collect(Vec<f64>),
}

/// Compile a custom-aggregation expression. The parser walks the AST and enumerates every
/// top-level aggregator call (`sum(...)`, `avg(...)`, …). Nested aggregators (e.g.
/// `sum(sum(V{value}))`) reject — DHIS2 does not permit them.
pub fn compile_custom_aggregation(src: &str) -> DapResult<CustomAggregationPlan> {
    let ast = compile(src)?;
    let mut aggregators = Vec::new();
    collect_aggregators(&ast, &mut aggregators)?;
    // Validate non-aggregator function calls with the usual whitelist. Aggregator function
    // names are not in SUPPORTED_FUNCTIONS; strip them before the walk.
    validate_non_aggregator_calls(&ast)?;
    Ok(CustomAggregationPlan {
        source: src.to_string(),
        ast,
        aggregators,
    })
}

fn collect_aggregators(
    ast: &ExpressionAst,
    out: &mut Vec<AggregatorKind>,
) -> DapResult<()> {
    match ast {
        ExpressionAst::Call { name, args } if is_aggregator(name) => {
            let kind = AggregatorKind::parse(name, args)?;
            out.push(kind);
            for a in args {
                reject_nested_aggregators(a)?;
            }
            Ok(())
        }
        ExpressionAst::Call { args, .. } => {
            for a in args {
                collect_aggregators(a, out)?;
            }
            Ok(())
        }
        ExpressionAst::Unary { inner, .. } => collect_aggregators(inner, out),
        ExpressionAst::Binary { lhs, rhs, .. } => {
            collect_aggregators(lhs, out)?;
            collect_aggregators(rhs, out)
        }
        _ => Ok(()),
    }
}

fn reject_nested_aggregators(ast: &ExpressionAst) -> DapResult<()> {
    match ast {
        ExpressionAst::Call { name, .. } if is_aggregator(name) => {
            Err(DapError::invalid_value(format!(
                "nested aggregator '{name}' inside another aggregator is not allowed"
            )))
        }
        ExpressionAst::Call { args, .. } => {
            for a in args {
                reject_nested_aggregators(a)?;
            }
            Ok(())
        }
        ExpressionAst::Unary { inner, .. } => reject_nested_aggregators(inner),
        ExpressionAst::Binary { lhs, rhs, .. } => {
            reject_nested_aggregators(lhs)?;
            reject_nested_aggregators(rhs)
        }
        _ => Ok(()),
    }
}

fn validate_non_aggregator_calls(ast: &ExpressionAst) -> DapResult<()> {
    match ast {
        ExpressionAst::Call { name, args } => {
            if !is_aggregator(name) {
                validate_functions(ast)?;
            }
            for a in args {
                validate_non_aggregator_calls(a)?;
            }
            Ok(())
        }
        ExpressionAst::Unary { inner, .. } => validate_non_aggregator_calls(inner),
        ExpressionAst::Binary { lhs, rhs, .. } => {
            validate_non_aggregator_calls(lhs)?;
            validate_non_aggregator_calls(rhs)
        }
        _ => Ok(()),
    }
}

fn is_aggregator(name: &str) -> bool {
    matches!(
        name,
        "sum" | "avg" | "average" | "count" | "min" | "max" | "stddev" | "variance" | "percentile"
    )
}

/// Evaluate a compiled custom aggregation across an input stream. Each aggregator call is
/// backed by its own online state; the outer expression is recomputed at the end with the
/// aggregators' finalised values substituted.
pub fn evaluate_custom_aggregation(
    plan: &CustomAggregationPlan,
    stream: impl IntoIterator<Item = Option<f64>>,
) -> DapResult<EValue> {
    // Each aggregator in `plan.aggregators` corresponds to a specific Call node in the AST.
    // We walk the AST, recording each aggregator Call -> state slot by stable position. The
    // walk order matches `collect_aggregators`.
    let mut states: Vec<AggState> = plan
        .aggregators
        .iter()
        .map(AggregatorKind::init_state)
        .collect();

    for x in stream.into_iter().flatten() {
        for (i, kind) in plan.aggregators.iter().enumerate() {
            update_state(&mut states[i], *kind, x);
        }
    }

    let finals: Vec<Option<f64>> = plan
        .aggregators
        .iter()
        .zip(states)
        .map(|(k, s)| finalize(*k, s))
        .collect();

    let mut cursor = 0usize;
    let mut resolver = StaticResolver { v: EValue::Null };
    eval_with_substitution(&plan.ast, &finals, &mut cursor, &mut resolver)
}

fn update_state(state: &mut AggState, kind: AggregatorKind, x: f64) {
    match (state, kind) {
        (AggState::Sum { sum, count }, _) => {
            *sum += x;
            *count += 1;
        }
        (AggState::Count(c), _) => *c += 1,
        (AggState::Avg { sum, count }, _) => {
            *sum += x;
            *count += 1;
        }
        (AggState::Min(m), _) => {
            if x < *m {
                *m = x;
            }
        }
        (AggState::Max(m), _) => {
            if x > *m {
                *m = x;
            }
        }
        (AggState::Welford { count, mean, m2 }, _) => {
            *count += 1;
            let delta = x - *mean;
            *mean += delta / *count as f64;
            let delta2 = x - *mean;
            *m2 += delta * delta2;
        }
        (AggState::Collect(v), _) => v.push(x),
    }
}

fn finalize(kind: AggregatorKind, state: AggState) -> Option<f64> {
    match (kind, state) {
        (AggregatorKind::Sum, AggState::Sum { sum, count }) => (count > 0).then_some(sum),
        (AggregatorKind::Count, AggState::Count(c)) => Some(c as f64),
        (AggregatorKind::Avg, AggState::Avg { sum, count }) => {
            (count > 0).then(|| sum / count as f64)
        }
        (AggregatorKind::Min, AggState::Min(v)) if v.is_finite() => Some(v),
        (AggregatorKind::Max, AggState::Max(v)) if v.is_finite() => Some(v),
        (AggregatorKind::Variance, AggState::Welford { count, m2, .. }) => {
            (count > 0).then(|| m2 / count as f64)
        }
        (AggregatorKind::StdDev, AggState::Welford { count, m2, .. }) => {
            (count > 0).then(|| (m2 / count as f64).sqrt())
        }
        (AggregatorKind::Percentile(p_micros), AggState::Collect(mut v)) if !v.is_empty() => {
            v.sort_by(|a, b| a.total_cmp(b));
            let p = p_micros as f64 / 1_000_000.0;
            let rank = p * (v.len() as f64 - 1.0);
            let lo = rank.floor() as usize;
            let hi = rank.ceil() as usize;
            if lo == hi {
                Some(v[lo])
            } else {
                let frac = rank - lo as f64;
                Some(v[lo] * (1.0 - frac) + v[hi] * frac)
            }
        }
        _ => None,
    }
}

struct StaticResolver {
    v: EValue,
}

impl Resolver for StaticResolver {
    fn reference(&mut self, _r: &RefKind) -> DapResult<EValue> {
        Ok(self.v.clone())
    }
}

/// Walk `ast` and substitute aggregator calls with their finalised values. `cursor` tracks
/// the next unused slot in `finals` (walk order matches `collect_aggregators`).
fn eval_with_substitution(
    ast: &ExpressionAst,
    finals: &[Option<f64>],
    cursor: &mut usize,
    resolver: &mut dyn Resolver,
) -> DapResult<EValue> {
    match ast {
        ExpressionAst::Call { name, .. } if is_aggregator(name) => {
            let idx = *cursor;
            *cursor += 1;
            Ok(match finals[idx] {
                Some(n) => EValue::Number(n),
                None => EValue::Null,
            })
        }
        ExpressionAst::Call { name, args } => {
            let mut sub_args = Vec::with_capacity(args.len());
            for a in args {
                sub_args.push(ast_literal(eval_with_substitution(
                    a, finals, cursor, resolver,
                )?));
            }
            evaluate_expr(
                &ExpressionAst::Call {
                    name: name.clone(),
                    args: sub_args,
                },
                resolver,
            )
        }
        ExpressionAst::Unary { op, inner } => {
            let v = eval_with_substitution(inner, finals, cursor, resolver)?;
            evaluate_expr(
                &ExpressionAst::Unary {
                    op: *op,
                    inner: Box::new(ast_literal(v)),
                },
                resolver,
            )
        }
        ExpressionAst::Binary { op, lhs, rhs } => {
            let l = eval_with_substitution(lhs, finals, cursor, resolver)?;
            let r = eval_with_substitution(rhs, finals, cursor, resolver)?;
            evaluate_expr(
                &ExpressionAst::Binary {
                    op: *op,
                    lhs: Box::new(ast_literal(l)),
                    rhs: Box::new(ast_literal(r)),
                },
                resolver,
            )
        }
        other => evaluate_expr(other, resolver),
    }
}

fn ast_literal(v: EValue) -> ExpressionAst {
    match v {
        EValue::Null => ExpressionAst::Null,
        EValue::Number(n) => ExpressionAst::Number(n),
        EValue::Bool(b) => ExpressionAst::Bool(b),
        EValue::Str(s) => ExpressionAst::String(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(xs: &[f64]) -> Vec<Option<f64>> {
        xs.iter().copied().map(Some).collect()
    }

    #[test]
    fn sum_and_count_and_division() {
        let plan = compile_custom_aggregation("100 * sum(V{value}) / count(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&[1.0, 2.0, 3.0, 4.0])).unwrap();
        // sum = 10, count = 4, 100 * 10 / 4 = 250.
        assert_eq!(v, EValue::Number(250.0));
    }

    #[test]
    fn avg_variance_stddev_percentile() {
        let data = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let plan = compile_custom_aggregation("avg(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&data))
            .unwrap()
            .as_number()
            .unwrap();
        assert!((v - 5.0).abs() < 1e-9);

        let plan = compile_custom_aggregation("variance(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&data))
            .unwrap()
            .as_number()
            .unwrap();
        assert!((v - 4.0).abs() < 1e-9);

        let plan = compile_custom_aggregation("stddev(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&data))
            .unwrap()
            .as_number()
            .unwrap();
        assert!((v - 2.0).abs() < 1e-9);

        let plan = compile_custom_aggregation("percentile(V{value}, 0.5)").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&data))
            .unwrap()
            .as_number()
            .unwrap();
        assert!((v - 4.5).abs() < 1e-9, "median = {v}");
    }

    #[test]
    fn nested_aggregator_rejected() {
        let err = compile_custom_aggregation("sum(sum(V{value}))").expect_err("nested rejected");
        assert!(err.to_string().contains("nested aggregator"));
    }

    #[test]
    fn unknown_aggregator_rejected_at_compile() {
        let err = compile_custom_aggregation("quantile(V{value}, 0.5)")
            .expect_err("unknown aggregator rejected");
        assert!(err.to_string().contains("quantile"));
    }

    #[test]
    fn null_stream_yields_null_per_aggregator() {
        let plan = compile_custom_aggregation("avg(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, vec![None, None, None]).unwrap();
        assert_eq!(v, EValue::Null);
    }
}
