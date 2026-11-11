//! Logical and physical plans.
//!
//! The logical plan is DSL-independent — the DHIS2 semantic compiler (dap-dhis2) and future
//! SQL compiler both target it.
//!
//! The physical plan selects concrete runtime operators and carries scalar push-down /
//! projection pruning decisions. We keep the physical plan shallow (no join reorder in v1):
//! `Scan -> Filter -> HashAggregate -> Sort(opt) -> Limit(opt)`.

use std::sync::Arc;

use dap_catalog::Catalog;
use dap_core::{DapError, DapResult, PeriodType, Schema, Value};
use dap_runtime::aggregate::AggregateKind;
use dap_runtime::expr::{CmpOp, Expr, Predicate};
use dap_runtime::sort::SortSpec;
use dap_storage::Partition;

use crate::dsl::{
    AggregateQuery, AggregationSpec, FilterSpec, OrderSpec, OrgUnitSelector, PeriodSpec,
    QueryRequest,
};

#[derive(Clone, Debug)]
pub struct PlanOptions {
    /// Enable zone-map / segment pruning. Default true; opt-out used by equivalence tests.
    pub enable_pruning: bool,
}

impl Default for PlanOptions {
    fn default() -> Self {
        Self { enable_pruning: true }
    }
}

#[derive(Clone, Debug)]
pub struct PlanContext {
    pub catalog: Arc<Catalog>,
    pub options: PlanOptions,
}

/// A logical plan is the metadata-bound form of a query. Everything the catalog needed to
/// resolve (OU subtree expansion, period range materialisation, column resolution) has
/// already happened.
#[derive(Clone, Debug)]
pub struct LogicalPlan {
    pub partition: Arc<Partition>,
    pub predicate: Predicate,
    pub group_by: Vec<String>,
    pub aggregates: Vec<AggregateKind>,
    pub order_by: Vec<SortSpec>,
    pub limit: Option<usize>,
    pub output_schema: Schema,
}

/// The physical plan is currently 1:1 with the logical plan. Future gates introduce
/// alternative join strategies and parallel fragmentation; the shape widens here.
pub type PhysicalPlan = LogicalPlan;

impl LogicalPlan {
    pub fn build(
        ctx: &PlanContext,
        request: &QueryRequest,
        partition: Arc<Partition>,
    ) -> DapResult<Self> {
        match request {
            QueryRequest::Aggregate(aq) => build_aggregate(ctx, aq, partition),
        }
    }
}

fn build_aggregate(
    ctx: &PlanContext,
    aq: &AggregateQuery,
    partition: Arc<Partition>,
) -> DapResult<LogicalPlan> {
    // 1. Validate the dataset in the catalog. The dataset's period type must match any Period
    //    filter the request carries (fail closed otherwise).
    let dataset = ctx.catalog.dataset(&aq.dataset)?;
    if let Some(PeriodSpec::Range { period_type, .. }) = &aq.period {
        if *period_type != dataset.period_type {
            return Err(DapError::invalid_value(format!(
                "period range uses {:?}, dataset '{}' expects {:?}",
                period_type, dataset.name, dataset.period_type
            )));
        }
    }

    // 2. Resolve group-by + aggregate columns against the partition schema.
    let schema = partition.schema().clone();
    for g in &aq.group_by {
        schema.column(g)?;
    }

    // 3. Compose the predicate.
    let mut terms: Vec<Predicate> = Vec::new();
    if let Some(p) = &aq.period {
        terms.push(period_predicate(p, dataset.period_type)?);
    }
    if let Some(ou) = &aq.org_unit {
        terms.push(org_unit_predicate(&ctx.catalog, ou)?);
    }
    for f in &aq.filters {
        terms.push(filter_predicate(f)?);
    }
    let predicate = if terms.is_empty() {
        Predicate::True
    } else if terms.len() == 1 {
        terms.pop().unwrap()
    } else {
        Predicate::And(terms)
    };

    // 4. Aggregates.
    let aggregates: Vec<AggregateKind> = aq
        .measures
        .iter()
        .map(convert_aggregate)
        .collect::<DapResult<Vec<_>>>()?;

    // 5. Output schema = group_by columns + aggregate output columns.
    let mut output_cols = Vec::new();
    for g in &aq.group_by {
        output_cols.push(schema.column(g)?.clone());
    }
    for a in &aggregates {
        output_cols.push(dap_core::Column::new(
            a.output_name(),
            a.output_type(&schema)?,
        ));
    }
    let output_schema = Schema::new(output_cols)?;

    // 6. Sort spec and limit.
    let order_by = aq.order_by.iter().map(convert_sort).collect();

    Ok(LogicalPlan {
        partition,
        predicate,
        group_by: aq.group_by.clone(),
        aggregates,
        order_by,
        limit: aq.limit,
        output_schema,
    })
}

fn period_predicate(spec: &PeriodSpec, dataset_pt: PeriodType) -> DapResult<Predicate> {
    // Period columns are physically stored as Utf8 (see `LogicalType::Period::default_physical`),
    // so the vectorized reader materialises them as `Value::Str`. The predicate side must use
    // the same physical shape, otherwise the typed-tag ordering in `Value::cmp` keeps the sides
    // disjoint. The value is still validated as a canonical DHIS2 period code first.
    match spec {
        PeriodSpec::Exact(code) => {
            let _ = dap_core::Period::parse(code)?;
            Ok(Predicate::Compare {
                left: Expr::Col("period".into()),
                op: CmpOp::Eq,
                right: Expr::Lit(Value::Str(code.clone())),
            })
        }
        PeriodSpec::In(codes) => {
            let mut values = Vec::with_capacity(codes.len());
            for c in codes {
                let _ = dap_core::Period::parse(c)?;
                values.push(Value::Str(c.clone()));
            }
            Ok(Predicate::In {
                expr: Expr::Col("period".into()),
                values,
            })
        }
        PeriodSpec::Range {
            period_type,
            start,
            end,
        } => {
            if *period_type != dataset_pt {
                return Err(DapError::invalid_value(format!(
                    "period range type {:?} != dataset {:?}",
                    period_type, dataset_pt
                )));
            }
            let s = dap_core::Period::parse(start)?;
            let e = dap_core::Period::parse(end)?;
            if s.period_type() != *period_type || e.period_type() != *period_type {
                return Err(DapError::invalid_value(
                    "range endpoints must share declared period_type",
                ));
            }
            // Lexicographic ordering matches chronological ordering for the DHIS2 canonical
            // codes we support (YYYY, YYYYMM, YYYYQn, YYYYWnn, YYYYMMDD).
            Ok(Predicate::And(vec![
                Predicate::Compare {
                    left: Expr::Col("period".into()),
                    op: CmpOp::GtEq,
                    right: Expr::Lit(Value::Str(start.clone())),
                },
                Predicate::Compare {
                    left: Expr::Col("period".into()),
                    op: CmpOp::LtEq,
                    right: Expr::Lit(Value::Str(end.clone())),
                },
            ]))
        }
    }
}

fn org_unit_predicate(catalog: &Catalog, sel: &OrgUnitSelector) -> DapResult<Predicate> {
    let expanded: Vec<dap_core::Uid> = match sel {
        OrgUnitSelector::In(uids) => uids.clone(),
        OrgUnitSelector::Subtree(roots) => {
            let mut set: ahash::AHashSet<dap_core::Uid> = Default::default();
            for r in roots {
                for u in catalog.org_unit_subtree(r) {
                    set.insert(u);
                }
            }
            set.into_iter().collect()
        }
    };
    let values = expanded
        .into_iter()
        .map(|u| Value::Str(u.as_str().to_string()))
        .collect();
    Ok(Predicate::In {
        expr: Expr::Col("ou".into()),
        values,
    })
}

fn filter_predicate(f: &FilterSpec) -> DapResult<Predicate> {
    Ok(match f {
        FilterSpec::Eq { column, value } => Predicate::Compare {
            left: Expr::Col(column.clone()),
            op: CmpOp::Eq,
            right: Expr::Lit(value.clone()),
        },
        FilterSpec::NotEq { column, value } => Predicate::Compare {
            left: Expr::Col(column.clone()),
            op: CmpOp::NotEq,
            right: Expr::Lit(value.clone()),
        },
        FilterSpec::Lt { column, value } => Predicate::Compare {
            left: Expr::Col(column.clone()),
            op: CmpOp::Lt,
            right: Expr::Lit(value.clone()),
        },
        FilterSpec::LtEq { column, value } => Predicate::Compare {
            left: Expr::Col(column.clone()),
            op: CmpOp::LtEq,
            right: Expr::Lit(value.clone()),
        },
        FilterSpec::Gt { column, value } => Predicate::Compare {
            left: Expr::Col(column.clone()),
            op: CmpOp::Gt,
            right: Expr::Lit(value.clone()),
        },
        FilterSpec::GtEq { column, value } => Predicate::Compare {
            left: Expr::Col(column.clone()),
            op: CmpOp::GtEq,
            right: Expr::Lit(value.clone()),
        },
        FilterSpec::In { column, values } => Predicate::In {
            expr: Expr::Col(column.clone()),
            values: values.clone(),
        },
        FilterSpec::IsNull { column } => Predicate::IsNull(Expr::Col(column.clone())),
        FilterSpec::IsNotNull { column } => Predicate::IsNotNull(Expr::Col(column.clone())),
    })
}

fn convert_aggregate(spec: &AggregationSpec) -> DapResult<AggregateKind> {
    Ok(match spec {
        AggregationSpec::CountStar => AggregateKind::CountStar,
        AggregationSpec::Count(c) => AggregateKind::CountNonNull { column: c.clone() },
        AggregationSpec::CountDistinct(c) => AggregateKind::CountDistinct { column: c.clone() },
        AggregationSpec::Sum(c) => AggregateKind::Sum { column: c.clone() },
        AggregationSpec::Min(c) => AggregateKind::Min { column: c.clone() },
        AggregationSpec::Max(c) => AggregateKind::Max { column: c.clone() },
        AggregationSpec::Avg(c) => AggregateKind::Avg { column: c.clone() },
    })
}

fn convert_sort(o: &OrderSpec) -> SortSpec {
    SortSpec {
        column: o.column.clone(),
        descending: o.descending,
        nulls_first: o.nulls_first,
    }
}
