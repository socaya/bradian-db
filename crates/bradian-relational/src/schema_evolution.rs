//! G27 additive-schema compatibility foundation for durable historical rows.
//!
//! An append-only nullable-column projection can be evaluated WITHOUT any
//! catalog or WAL change. This does NOT issue ALTER TABLE or publish a new
//! schema epoch: the G11 catalog and G22 history must be changed atomically
//! before an online DDL executor can exist.

use crate::{Column, Table};
use crate::typed_table::{validate_table, TableError, Value};

/// A proposed schema revision that preserves every existing column, primary
/// key and constraint and appends only nullable columns with no defaults.
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
    validate_table(previous)?;
    validate_table(proposed)?;
    if previous.id != proposed.id
        || previous.schema != proposed.schema
        || previous.name != proposed.name
        || previous.primary_key != proposed.primary_key
        || previous.unique_keys != proposed.unique_keys
        || previous.foreign_keys != proposed.foreign_keys
    {
        return Err(TableError::Schema(
            "schema evolution cannot change table identity or constraints".into(),
        ));
    }
    if proposed.columns.len() <= previous.columns.len()
        || !proposed.columns.starts_with(&previous.columns)
    {
        return Err(TableError::Schema(
            "only trailing nullable columns with unchanged existing definitions are supported".into(),
        ));
    }
    let added = proposed.columns[previous.columns.len()..].to_vec();
    if added.iter().any(|column| !column.nullable) {
        return Err(TableError::Schema(
            "new trailing columns must be nullable; defaults and backfills are unsupported".into(),
        ));
    }
    Ok(AdditiveSchemaPlan {
        previous: previous.clone(), proposed: proposed.clone(),
        added_columns: added,
    })
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
}
