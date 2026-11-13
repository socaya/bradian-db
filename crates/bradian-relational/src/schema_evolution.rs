//! G27/G39 nullable-schema validation and durable historical row projection.
//!
//! Planning here has no catalog or WAL side effects. G36's exclusive owner
//! coordinates publication and recovery of validated schema transitions.

use crate::{Column, Table};
use crate::typed_table::{validate_table, TableError, Value};

/// A compatible projection preserving existing positions, types and keys.
/// G27 appends nullable columns; G39 may also relax non-key NOT NULL columns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdditiveSchemaPlan {
    pub previous: Table,
    pub proposed: Table,
    pub added_columns: Vec<Column>,
}

/// Compute a safe, read-only compatibility plan. This intentionally rejects
/// renames, dropped/reordered columns, type changes, non-null additions and
/// any changes to primary/unique/foreign keys.
pub fn plan_nullable_additions(
    previous: &Table,
    proposed: &Table,
) -> Result<AdditiveSchemaPlan, TableError> {
    let plan = plan_nullable_evolution(previous, proposed)?;
    if proposed.columns.len() <= previous.columns.len()
        || !proposed.columns.starts_with(&previous.columns)
    {
        return Err(TableError::Schema(
            "only trailing nullable columns with unchanged existing definitions are supported".into(),
        ));
    }
    Ok(plan)
}

/// Monotone nullable evolution: preserve names, types, positions and keys,
/// permit NOT NULL relaxation on non-key columns and append nullable columns.
/// The original G27 planner above deliberately retains its stricter contract.
pub fn plan_nullable_evolution(
    previous: &Table, proposed: &Table,
) -> Result<AdditiveSchemaPlan, TableError> {
    validate_table(previous)?;
    validate_table(proposed)?;
    if previous.id != proposed.id || previous.schema != proposed.schema
        || previous.name != proposed.name || previous.primary_key != proposed.primary_key
        || previous.unique_keys != proposed.unique_keys || previous.foreign_keys != proposed.foreign_keys
    {
        return Err(TableError::Schema("schema evolution cannot change table identity or keys".into()));
    }
    if proposed == previous || proposed.columns.len() < previous.columns.len() {
        return Err(TableError::Schema("schema evolution requires a permitted change and cannot drop columns".into()));
    }
    for (old, new) in previous.columns.iter().zip(&proposed.columns) {
        if old.name != new.name || old.data_type != new.data_type || (old.nullable && !new.nullable) {
            return Err(TableError::Schema("only NOT NULL relaxation preserves existing column definitions".into()));
        }
    }
    let added_columns = proposed.columns[previous.columns.len()..].to_vec();
    if added_columns.iter().any(|column| !column.nullable) {
        return Err(TableError::Schema("new columns require nullable definitions; backfills are unsupported".into()));
    }
    Ok(AdditiveSchemaPlan { previous: previous.clone(), proposed: proposed.clone(), added_columns })
}

/// Read-only row projection into a proposed additive schema. This verifies
/// row width; callers must first decode the row with its ACTUAL source schema.
/// Never encode these projected rows as physical writes without a separate
/// migration transaction and version-aware row codec.
pub fn project_row(
    plan: &AdditiveSchemaPlan,
    old_row: &[Value],
) -> Result<Vec<Value>, TableError> {
    if old_row.len() != plan.previous.columns.len() {
        return Err(TableError::Value(
            "historical row width does not match its source schema".into(),
        ));
    }
    // Source validation also checks nullability, types and constraints.
    crate::typed_table::encode_row(&plan.previous, old_row)?;
    let mut projected = Vec::with_capacity(plan.proposed.columns.len());
    projected.extend_from_slice(old_row);
    projected.extend((0..plan.added_columns.len()).map(|_| Value::Null));
    Ok(projected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DataType, PrimaryKey, SchemaId, TableId};

    fn table() -> Table {
        Table {
            id: TableId(42), schema: SchemaId(3), name: "items".into(),
            columns: vec![
                Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
                Column { name: "amount".into(), data_type: DataType::Int64, nullable: false },
            ],
            primary_key: PrimaryKey { columns: vec!["id".into()] },
            unique_keys: vec![], foreign_keys: vec![],
        }
    }

    #[test]
    fn additive_nullable_projection_keeps_values_and_appends_null() {
        let old = table();
        let mut next = old.clone();
        next.columns.push(Column {
            name: "comment".into(), data_type: DataType::Text, nullable: true,
        });
        let plan = plan_nullable_additions(&old, &next).unwrap();
        assert_eq!(project_row(&plan, &[Value::Int64(1), Value::Int64(50)]).unwrap(),
            vec![Value::Int64(1), Value::Int64(50), Value::Null]);
        assert_eq!(old.columns.len(), 2);
    }

    #[test]
    fn rejects_non_nullable_reorder_and_type_changes() {
        let old = table();
        let mut next = old.clone();
        next.columns.push(Column {
            name: "required".into(), data_type: DataType::Text, nullable: false,
        });
        assert!(plan_nullable_additions(&old, &next).is_err());
        next = old.clone();
        next.columns.swap(0, 1);
        assert!(plan_nullable_additions(&old, &next).is_err());
        next = old.clone();
        next.columns[1].data_type = DataType::Text;
        next.columns.push(Column {
            name: "extra".into(), data_type: DataType::Text, nullable: true,
        });
        assert!(plan_nullable_additions(&old, &next).is_err());
    }

    #[test]
    fn refuses_row_shape_mismatch_and_invalid_values() {
        let old = table();
        let mut next = old.clone();
        next.columns.push(Column {
            name: "optional".into(), data_type: DataType::Text, nullable: true,
        });
        let plan = plan_nullable_additions(&old, &next).unwrap();
        assert!(project_row(&plan, &[Value::Int64(1)]).is_err());
        assert!(project_row(&plan, &[Value::Null, Value::Int64(50)]).is_err());
        assert!(project_row(&plan, &[Value::Int64(1), Value::String("x".into())]).is_err());
    }

    #[test]
    fn relaxation_preserves_values_but_rejects_primary_key_relaxation_and_narrowing() {
        let old = table();
        let mut next = old.clone();
        next.columns[1].nullable = true;
        let plan = plan_nullable_evolution(&old, &next).unwrap();
        assert_eq!(project_row(&plan, &[Value::Int64(1), Value::Int64(50)]).unwrap(), vec![Value::Int64(1), Value::Int64(50)]);
        assert!(plan_nullable_additions(&old, &next).is_err());
        assert!(plan_nullable_evolution(&next, &old).is_err());
        let mut key_change = old.clone();
        key_change.columns[0].nullable = true;
        assert!(plan_nullable_evolution(&old, &key_change).is_err());
        assert!(plan_nullable_evolution(&old, &old).is_err());
    }

    #[test]
    fn projection_handles_an_addition_and_relaxation_across_multiple_epochs() {
        let old = table();
        let mut latest = old.clone();
        latest.columns[1].nullable = true;
        latest.columns.push(Column { name: "note".into(), data_type: DataType::Text, nullable: true });
        let plan = plan_nullable_evolution(&old, &latest).unwrap();
        assert_eq!(project_row(&plan, &[Value::Int64(1), Value::Int64(50)]).unwrap(), vec![Value::Int64(1), Value::Int64(50), Value::Null]);
        assert!(project_row(&plan, &[Value::Int64(1), Value::Null]).is_err());
        assert!(plan_nullable_additions(&old, &latest).is_err());
    }
}
