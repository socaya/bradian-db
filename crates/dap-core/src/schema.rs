//! Column + schema definitions shared between storage, runtime and the catalog.

use serde::{Deserialize, Serialize};

use crate::error::{DapError, DapResult};
use crate::value::LogicalType;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub name: String,
    pub logical_type: LogicalType,
    #[serde(default = "nullable_default")]
    pub nullable: bool,
}

const fn nullable_default() -> bool {
    true
}

impl Column {
    pub fn new(name: impl Into<String>, logical_type: LogicalType) -> Self {
        Self {
            name: name.into(),
            logical_type,
            nullable: true,
        }
    }

    pub fn required(mut self) -> Self {
        self.nullable = false;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Schema {
    pub columns: Vec<Column>,
}

impl Schema {
    pub fn new(columns: Vec<Column>) -> DapResult<Self> {
        let mut seen = std::collections::HashSet::new();
        for c in &columns {
            if !seen.insert(c.name.as_str()) {
                return Err(DapError::schema_mismatch(format!(
                    "duplicate column name: {}",
                    c.name
                )));
            }
        }
        Ok(Self { columns })
    }

    pub fn index_of(&self, name: &str) -> DapResult<usize> {
        self.columns
            .iter()
            .position(|c| c.name == name)
            .ok_or_else(|| DapError::NotFound(format!("column {name}")))
    }

    pub fn column(&self, name: &str) -> DapResult<&Column> {
        let i = self.index_of(name)?;
        Ok(&self.columns[i])
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_columns_rejected() {
        let c = Column::new("a", LogicalType::I64);
        assert!(Schema::new(vec![c.clone(), c]).is_err());
    }

    #[test]
    fn index_of_lookup() {
        let s = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("period", LogicalType::Period).required(),
            Column::new("value", LogicalType::F64),
        ])
        .unwrap();
        assert_eq!(s.index_of("period").unwrap(), 1);
        assert!(s.index_of("missing").is_err());
    }
}
