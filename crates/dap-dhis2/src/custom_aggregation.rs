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
    /// Median: shorthand for `percentile(V{value}, 0.5)`.
    Median,
    /// Spearman rank autocorrelation across successive values of the stream.
    SpearmanAutocorr,
    /// Kendall tau-b rank autocorrelation across successive values.
    KendallAutocorr,
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
            ("median", [_]) => Ok(Self::Median),
            ("spearman_autocorr", [_]) => Ok(Self::SpearmanAutocorr),
            ("kendall_autocorr", [_]) => Ok(Self::KendallAutocorr),
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
            Self::Percentile(_) | Self::Median => AggState::Collect(Vec::new()),
            Self::SpearmanAutocorr | Self::KendallAutocorr => AggState::Collect(Vec::new()),
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
        "sum"
            | "avg"
            | "average"
            | "count"
            | "min"
            | "max"
            | "stddev"
            | "variance"
            | "percentile"
            | "median"
            | "spearman_autocorr"
            | "kendall_autocorr"
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
        (AggregatorKind::Median, AggState::Collect(mut v)) if !v.is_empty() => {
            v.sort_by(|a, b| a.total_cmp(b));
            let n = v.len();
            if n % 2 == 1 {
                Some(v[n / 2])
            } else {
                Some((v[n / 2 - 1] + v[n / 2]) / 2.0)
            }
        }
        (AggregatorKind::SpearmanAutocorr, AggState::Collect(v)) if v.len() >= 2 => {
            let pairs: Vec<(f64, f64)> = v.windows(2).map(|w| (w[0], w[1])).collect();
            Some(spearman_rho(&pairs))
        }
        (AggregatorKind::KendallAutocorr, AggState::Collect(v)) if v.len() >= 2 => {
            let pairs: Vec<(f64, f64)> = v.windows(2).map(|w| (w[0], w[1])).collect();
            Some(kendall_tau(&pairs))
        }
        _ => None,
    }
}

fn spearman_rho(pairs: &[(f64, f64)]) -> f64 {
    if pairs.len() < 2 {
        return f64::NAN;
    }
    let xs: Vec<f64> = pairs.iter().map(|p| p.0).collect();
    let ys: Vec<f64> = pairs.iter().map(|p| p.1).collect();
    let rx = midrank(&xs);
    let ry = midrank(&ys);
    pearson(&rx, &ry)
}

fn kendall_tau(pairs: &[(f64, f64)]) -> f64 {
    let n = pairs.len();
    if n < 2 {
        return f64::NAN;
    }
    let mut concordant = 0i64;
    let mut discordant = 0i64;
    let mut ties_x = 0i64;
    let mut ties_y = 0i64;
    for i in 0..n {
        for j in (i + 1)..n {
            let dx = pairs[i].0 - pairs[j].0;
            let dy = pairs[i].1 - pairs[j].1;
            if dx == 0.0 && dy == 0.0 {
                ties_x += 1;
                ties_y += 1;
            } else if dx == 0.0 {
                ties_x += 1;
            } else if dy == 0.0 {
                ties_y += 1;
            } else if dx.signum() == dy.signum() {
                concordant += 1;
            } else {
                discordant += 1;
            }
        }
    }
    let total = (n * (n - 1) / 2) as i64;
    let denom = (((total - ties_x) * (total - ties_y)) as f64).sqrt();
    if denom == 0.0 {
        0.0
    } else {
        (concordant - discordant) as f64 / denom
    }
}

fn midrank(xs: &[f64]) -> Vec<f64> {
    let n = xs.len();
    let mut indexed: Vec<(usize, f64)> = xs.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut ranks = vec![0.0; n];
    let mut i = 0;
    while i < n {
        let mut j = i + 1;
        while j < n && indexed[j].1 == indexed[i].1 {
            j += 1;
        }
        let rank = (i + 1 + j) as f64 / 2.0;
        for k in i..j {
            ranks[indexed[k].0] = rank;
        }
        i = j;
    }
    ranks
}

fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    let mean_x = xs.iter().sum::<f64>() / n;
    let mean_y = ys.iter().sum::<f64>() / n;
    let mut num = 0.0;
    let mut dx2 = 0.0;
    let mut dy2 = 0.0;
    for (x, y) in xs.iter().zip(ys.iter()) {
        num += (x - mean_x) * (y - mean_y);
        dx2 += (x - mean_x).powi(2);
        dy2 += (y - mean_y).powi(2);
    }
    let denom = (dx2 * dy2).sqrt();
    if denom == 0.0 {
        0.0
    } else {
        num / denom
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
    fn median_matches_middle_value() {
        let plan = compile_custom_aggregation("median(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&[1.0, 3.0, 2.0, 5.0, 4.0]))
            .unwrap()
            .as_number()
            .unwrap();
        assert_eq!(v, 3.0);
    }

    #[test]
    fn spearman_autocorr_detects_monotonic_series() {
        // Strictly increasing values pair as (1,2), (2,3), (3,4), (4,5) → perfect positive
        // Spearman rank correlation of 1.0.
        let plan = compile_custom_aggregation("spearman_autocorr(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&[1.0, 2.0, 3.0, 4.0, 5.0]))
            .unwrap()
            .as_number()
            .unwrap();
        assert!((v - 1.0).abs() < 1e-9, "got {v}");
    }

    #[test]
    fn kendall_autocorr_detects_decreasing_series() {
        let plan = compile_custom_aggregation("kendall_autocorr(V{value})").unwrap();
        let v = evaluate_custom_aggregation(&plan, stream(&[5.0, 4.0, 3.0, 2.0, 1.0]))
            .unwrap()
            .as_number()
            .unwrap();
        assert!((v - 1.0).abs() < 1e-9, "got {v}");
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
