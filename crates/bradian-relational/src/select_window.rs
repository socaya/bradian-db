//! G44: bound and apply a historical SELECT's typed ordering and row window.
use std::cmp::Ordering;
use std::collections::BTreeSet;
use crate::{DataType, Table};
use crate::sql::{locate, Parser, SqlError, SqlResult};
use crate::typed_table::Value;

const MAX_SORT_KEYS: usize = 16;
const MAX_INPUT_ROWS: usize = 10000;
const MAX_LIMIT: u64 = 10000;
const MAX_OFFSET: u64 = 1000000;

struct SortKey {
    position: usize,
    descending: bool,
    nulls_first: bool,
}

pub(crate) struct SelectWindow {
    keys: Vec<SortKey>,
    primary_key: Vec<usize>,
    limit: Option<usize>,
    offset: usize,
    active: bool,
}

impl SelectWindow {
    pub(crate) fn parse(p: &mut Parser, table: &Table) -> SqlResult<Self> {
        let mut keys = Vec::new();
        let mut seen = BTreeSet::new();
        let ordered = p.consume_word("order");
        if ordered {
            p.expect("by")?;
            loop {
                let position = locate(table, &p.word()?)?;
                if !seen.insert(position) { return Err(SqlError::Syntax("duplicate ORDER BY column".into())); }
                if keys.len() == MAX_SORT_KEYS { return Err(SqlError::Unsupported("ORDER BY exceeds 16 columns".into())); }
                let descending = if p.consume_word("desc") { true } else { p.consume_word("asc"); false };
                let nulls_first = if p.consume_word("nulls") {
                    if p.consume_word("first") { true }
                    else { p.expect("last")?; false }
                } else { descending };
                keys.push(SortKey { position, descending, nulls_first });
                if !p.consume_symbol(',') { break; }
            }
        }
        let limit = if p.consume_word("limit") {
            let value = p.number_u64()?;
            if value > MAX_LIMIT { return Err(SqlError::Unsupported("LIMIT exceeds 10000".into())); }
            Some(value as usize)
        } else { None };
        let offset_clause = p.consume_word("offset");
        let offset = if offset_clause {
            let value = p.number_u64()?;
            if value > MAX_OFFSET { return Err(SqlError::Unsupported("OFFSET exceeds 1000000".into())); }
            value as usize
        } else { 0 };
        let primary_key = table.primary_key.columns.iter().map(|name| locate(table, name)).collect::<SqlResult<Vec<_>>>()?;
        Ok(Self { keys, primary_key, limit, offset, active: ordered || limit.is_some() || offset_clause })
    }

    pub(crate) fn input_row_limit(&self) -> Option<usize> { self.active.then_some(MAX_INPUT_ROWS) }

    pub(crate) fn apply(self, mut rows: Vec<Vec<Value>>, table: &Table) -> SqlResult<Vec<Vec<Value>>> {
        if !self.active { return Ok(rows); }
        if rows.len() > MAX_INPUT_ROWS { return Err(SqlError::Unsupported("ordered/windowed SELECT exceeds 10000 input rows".into())); }
        // Validate comparator preconditions once, outside the infallible sort callback.
        for row in &rows {
            if row.len() != table.columns.len() { return Err(SqlError::Type("query row has incompatible width".into())); }
            for key in self.keys.iter().map(|key| key.position).chain(self.primary_key.iter().copied()) {
                if !matches_type(&row[key], &table.columns[key].data_type) {
                    return Err(SqlError::Type("query sort value has incompatible type".into()));
                }
            }
        }
        rows.sort_by(|left, right| {
            for key in &self.keys {
                let order = compare(&left[key.position], &right[key.position], key.descending, key.nulls_first);
                if order != Ordering::Equal { return order; }
            }
            for position in &self.primary_key {
                let order = compare(&left[*position], &right[*position], false, false);
                if order != Ordering::Equal { return order; }
            }
            Ordering::Equal
        });
        Ok(rows.into_iter().skip(self.offset).take(self.limit.unwrap_or(usize::MAX)).collect())
    }
}

fn matches_type(value: &Value, data_type: &DataType) -> bool {
    matches!((value, data_type), (Value::Null, _)
        | (Value::Boolean(_), DataType::Boolean)
        | (Value::Int32(_), DataType::Int32)
        | (Value::Int64(_), DataType::Int64)
        | (Value::Decimal(_), DataType::Decimal { .. })
        | (Value::String(_), DataType::Text | DataType::Varchar { .. })
        | (Value::TimestampTz(_), DataType::TimestampTz)
        | (Value::Uuid(_), DataType::Uuid))
}

fn compare(left: &Value, right: &Value, descending: bool, nulls_first: bool) -> Ordering {
    match (left, right) {
        (Value::Null, Value::Null) => return Ordering::Equal,
        (Value::Null, _) => return if nulls_first { Ordering::Less } else { Ordering::Greater },
        (_, Value::Null) => return if nulls_first { Ordering::Greater } else { Ordering::Less },
        _ => {},
    }
    let result = match (left, right) {
        (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b),
        (Value::Int32(a), Value::Int32(b)) => a.cmp(b),
        (Value::Int64(a), Value::Int64(b)) => a.cmp(b),
        (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        (Value::TimestampTz(a), Value::TimestampTz(b)) => a.cmp(b),
        (Value::Uuid(a), Value::Uuid(b)) => a.cmp(b),
        _ => unreachable!("sort values were checked against the same column type"),
    };
    if descending { result.reverse() } else { result }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Column, PrimaryKey, SchemaId, TableId};

    fn table(data_type: DataType) -> Table {
        Table { id: TableId(1), schema: SchemaId(1), name: "items".into(),
            columns: vec![Column { name: "id".into(), data_type: DataType::Int64, nullable: false },
                Column { name: "value".into(), data_type, nullable: true }],
            primary_key: PrimaryKey { columns: vec!["id".into()] }, unique_keys: vec![], foreign_keys: vec![], column_defaults: Default::default() }
    }

    #[test]
    fn native_timestamp_and_uuid_ordering_use_instant_and_binary_value() {
        for (data_type, low, high) in [
            (DataType::TimestampTz, Value::TimestampTz(-1), Value::TimestampTz(1)),
            (DataType::Uuid, Value::Uuid([0;16]), Value::Uuid([255;16])),
        ] {
            let table = table(data_type);
            let mut parser = Parser::new_expressions("ORDER BY value DESC NULLS LAST").unwrap();
            let window = SelectWindow::parse(&mut parser, &table).unwrap();
            parser.end().unwrap();
            let rows = window.apply(vec![vec![Value::Int64(1),low], vec![Value::Int64(2),Value::Null],
                vec![Value::Int64(3),high]], &table).unwrap();
            assert_eq!(rows.iter().map(|row|row[0].clone()).collect::<Vec<_>>(), vec![Value::Int64(3),Value::Int64(1),Value::Int64(2)]);
        }
    }

    #[test]
    fn composite_primary_key_ties_follow_declared_key_order_not_column_layout() {
        let mut table = table(DataType::Int64);
        table.columns.push(Column { name: "second".into(), data_type: DataType::Int64, nullable: false });
        table.primary_key.columns = vec!["second".into(), "id".into()];
        let mut parser = Parser::new_expressions("ORDER BY value").unwrap();
        let window = SelectWindow::parse(&mut parser, &table).unwrap();
        let row = |id, second| vec![Value::Int64(id), Value::Int64(5), Value::Int64(second)];
        assert_eq!(window.apply(vec![row(1,2),row(3,-1),row(2,-1)], &table).unwrap(),
            vec![row(2,-1),row(3,-1),row(1,2)]);
    }
}
