//! Typed row batch — wraps `ColumnBuffer` arrays with explicit schema references.
//!
//! The batch is the runtime's hot path type. Operators accept and produce `RecordBatch`
//! values; batching keeps allocation + branching amortised across many rows.

use dap_core::{DapError, DapResult, Schema, Value};
use dap_storage::ColumnBuffer;

#[derive(Debug, Clone)]
pub struct RecordBatch {
    pub schema: Schema,
    pub columns: Vec<ColumnBuffer>,
    pub rows: usize,
}

impl RecordBatch {
    pub fn new(schema: Schema, columns: Vec<ColumnBuffer>) -> DapResult<Self> {
        if columns.len() != schema.len() {
            return Err(DapError::schema_mismatch(format!(
                "schema has {} columns, received {}",
                schema.len(),
                columns.len()
            )));
        }
        let rows = columns.first().map(|c| c.len()).unwrap_or(0);
        for (i, c) in columns.iter().enumerate() {
            if c.len() != rows {
                return Err(DapError::schema_mismatch(format!(
                    "column {} has {} rows, expected {}",
                    i,
                    c.len(),
                    rows
                )));
            }
        }
        Ok(Self { schema, columns, rows })
    }

    pub fn empty(schema: Schema) -> Self {
        Self { schema, columns: Vec::new(), rows: 0 }
    }

    pub fn column(&self, name: &str) -> DapResult<&ColumnBuffer> {
        let idx = self.schema.index_of(name)?;
        Ok(&self.columns[idx])
    }

    pub fn column_at(&self, idx: usize) -> &ColumnBuffer {
        &self.columns[idx]
    }

    pub fn row_count(&self) -> usize {
        self.rows
    }

    pub fn is_empty(&self) -> bool {
        self.rows == 0
    }

    /// Materialise as rows of `Value`. Used by the scalar oracle and by the API serialisation
    /// path. Not called on hot paths.
    pub fn row_values(&self, row: usize) -> DapResult<Vec<Value>> {
        let mut out = Vec::with_capacity(self.columns.len());
        for c in &self.columns {
            out.push(c.value_at(row)?);
        }
        Ok(out)
    }
}

/// Builder used by scan/materialisation paths that assemble batches from typed inputs.
#[derive(Debug)]
pub struct RecordBatchBuilder {
    schema: Schema,
    builders: Vec<dap_storage::column::ColumnBuilder>,
}

impl RecordBatchBuilder {
    pub fn new(schema: Schema) -> Self {
        let builders = schema
            .columns
            .iter()
            .map(|c| dap_storage::column::ColumnBuilder::new(c.logical_type))
            .collect();
        Self { schema, builders }
    }

    pub fn append_row(&mut self, row: &[Value]) -> DapResult<()> {
        if row.len() != self.schema.len() {
            return Err(DapError::schema_mismatch("row width mismatch"));
        }
        for (i, v) in row.iter().enumerate() {
            self.builders[i].append_value(v)?;
        }
        Ok(())
    }

    pub fn finish(self) -> DapResult<RecordBatch> {
        let columns: Vec<ColumnBuffer> = self.builders.into_iter().map(|b| b.finish()).collect();
        RecordBatch::new(self.schema, columns)
    }

    pub fn len(&self) -> usize {
        self.builders.first().map(|b| b.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_core::{Column, LogicalType};

    #[test]
    fn build_and_read_batch() {
        let schema = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("value", LogicalType::F64),
        ])
        .unwrap();
        let mut b = RecordBatchBuilder::new(schema.clone());
        b.append_row(&[Value::Str("a".into()), Value::F64(1.0)]).unwrap();
        b.append_row(&[Value::Str("b".into()), Value::Null]).unwrap();
        let batch = b.finish().unwrap();
        assert_eq!(batch.row_count(), 2);
        assert_eq!(batch.row_values(1).unwrap()[1], Value::Null);
    }
}
