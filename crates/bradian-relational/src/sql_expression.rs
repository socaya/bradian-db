//! Bounded expressions for historical exact-primary-key UPDATE only.
use crate::{Column, DataType, Table};
use crate::sql::{coerce, locate, Literal, Parser, SqlError, SqlResult};
use crate::typed_table::Value;

const MAX_NODES: usize = 128;
const MAX_DEPTH: usize = 32;

pub(crate) enum Expression {
    Literal(Literal),
    Column(String),
    Default,
    Unary(char, Box<Expression>),
    Binary(char, Box<Expression>, Box<Expression>),
}

fn spend(nodes: &mut usize) -> SqlResult<()> {
    *nodes += 1;
    if *nodes > MAX_NODES { return Err(SqlError::Unsupported("UPDATE expression node budget exceeded".into())); }
    Ok(())
}

pub(crate) fn assignments(p: &mut Parser) -> SqlResult<Vec<(String, Expression)>> {
    let mut nodes = 0;
    let mut fields = Vec::new();
    loop {
        let name = p.word()?;
        p.symbol('=')?;
        fields.push((name, sum(p, &mut nodes, 0)?));
        if !p.consume_symbol(',') { break; }
    }
    Ok(fields)
}

fn sum(p: &mut Parser, nodes: &mut usize, depth: usize) -> SqlResult<Expression> {
    let mut value = product(p, nodes, depth)?;
    loop {
        let operator = if p.consume_symbol('+') { '+' } else if p.consume_symbol('-') { '-' } else { break; };
        spend(nodes)?;
        value = Expression::Binary(operator, Box::new(value), Box::new(product(p, nodes, depth)?));
    }
    Ok(value)
}

fn product(p: &mut Parser, nodes: &mut usize, depth: usize) -> SqlResult<Expression> {
    let mut value = atom(p, nodes, depth)?;
    loop {
        let operator = if p.consume_symbol('*') { '*' } else if p.consume_symbol('/') { '/' } else { break; };
        spend(nodes)?;
        value = Expression::Binary(operator, Box::new(value), Box::new(atom(p, nodes, depth)?));
    }
    Ok(value)
}

fn atom(p: &mut Parser, nodes: &mut usize, depth: usize) -> SqlResult<Expression> {
    if depth > MAX_DEPTH { return Err(SqlError::Unsupported("UPDATE expression nesting budget exceeded".into())); }
    if p.consume_symbol('(') {
        let value = sum(p, nodes, depth + 1)?;
        p.symbol(')')?;
        return Ok(value);
    }
    spend(nodes)?;
    // Fold a signed numeric literal before coercion, including i32/i64 minima.
    if p.next_is_literal() { return Ok(Expression::Literal(p.literal()?)); }
    if p.consume_word("default") { return Ok(Expression::Default); }
    let sign = if p.consume_symbol('-') { Some('-') } else if p.consume_symbol('+') { Some('+') } else { None };
    if let Some(sign) = sign { return Ok(Expression::Unary(sign, Box::new(atom(p, nodes, depth + 1)?))); }
    Ok(Expression::Column(p.word()?))
}

pub(crate) enum BoundExpression {
    Value(Value),
    Column(usize),
    Unary(char, Box<BoundExpression>),
    Binary(char, Box<BoundExpression>, Box<BoundExpression>),
}

fn numeric(column: &Column) -> SqlResult<()> {
    if matches!(column.data_type, DataType::Int32 | DataType::Int64 | DataType::Decimal { .. }) { Ok(()) }
    else { Err(SqlError::Type("arithmetic requires an integer or fixed-scale decimal target".into())) }
}

pub(crate) fn bind(table: &Table, position: usize, expression: Expression) -> SqlResult<BoundExpression> {
    bind_inner(table, &table.columns[position], expression, true)
}

fn bind_inner(table: &Table, target: &Column, expression: Expression, root: bool) -> SqlResult<BoundExpression> {
    Ok(match expression {
        Expression::Literal(literal) => {
            let mut column = target.clone();
            // NULL intermediates propagate; final row validation enforces NOT NULL.
            if !root { column.nullable = true; }
            BoundExpression::Value(coerce(&literal, &column)?)
        },
        Expression::Default => {
            if !root { return Err(SqlError::Unsupported("DEFAULT must be a whole assignment".into())); }
            BoundExpression::Value(match table.column_defaults.get(&target.name) {
                Some(value) => value.clone(),
                None if target.nullable => Value::Null,
                None => return Err(SqlError::Type("required column has no default".into())),
            })
        },
        Expression::Column(name) => {
            let position = locate(table, &name)?;
            if table.columns[position].data_type != target.data_type {
                return Err(SqlError::Type("expression columns must match the target data type exactly".into()));
            }
            BoundExpression::Column(position)
        },
        Expression::Unary(operator, value) => {
            numeric(target)?;
            BoundExpression::Unary(operator, Box::new(bind_inner(table, target, *value, false)?))
        },
        Expression::Binary(operator, left, right) => {
            numeric(target)?;
            BoundExpression::Binary(operator, Box::new(bind_inner(table, target, *left, false)?),
                Box::new(bind_inner(table, target, *right, false)?))
        },
    })
}

fn arithmetic_error(message: &str) -> SqlError { SqlError::Type(format!("UPDATE arithmetic: {message}")) }

fn coefficient(value: Value) -> SqlResult<Option<i128>> {
    match value {
        Value::Null => Ok(None),
        Value::Int32(v) => Ok(Some(i128::from(v))),
        Value::Int64(v) => Ok(Some(i128::from(v))),
        Value::Decimal(v) => Ok(Some(v)),
        _ => Err(arithmetic_error("non-numeric operand")),
    }
}

fn pack(value: i128, target: &Column) -> SqlResult<Value> {
    let overflow = || arithmetic_error("result exceeds declared numeric bounds");
    match target.data_type {
        DataType::Int32 => Ok(Value::Int32(value.try_into().map_err(|_| overflow())?)),
        DataType::Int64 => Ok(Value::Int64(value.try_into().map_err(|_| overflow())?)),
        DataType::Decimal { precision, .. } if value.unsigned_abs() < 10_u128.pow(u32::from(precision)) => Ok(Value::Decimal(value)),
        _ => Err(overflow()),
    }
}

fn gcd(mut a: u128, mut b: u128) -> u128 {
    while b != 0 { (a, b) = (b, a % b); }
    a
}

fn decimal_multiply(a: i128, b: i128, factor: i128) -> SqlResult<i128> {
    let cancel = gcd(a.unsigned_abs(), factor as u128) as i128;
    let a = a / cancel;
    let factor = factor / cancel;
    let cancel = gcd(b.unsigned_abs(), factor as u128) as i128;
    if factor / cancel != 1 { return Err(arithmetic_error("result requires more fractional digits than the declared scale")); }
    a.checked_mul(b / cancel).ok_or_else(|| arithmetic_error("numeric overflow"))
}

fn decimal_divide(a: i128, b: i128, factor: i128) -> SqlResult<i128> {
    if b == 0 { return Err(arithmetic_error("division by zero")); }
    let cancel = gcd(a.unsigned_abs(), b.unsigned_abs()) as i128;
    let a = a / cancel;
    let b = b / cancel;
    let cancel = gcd(factor as u128, b.unsigned_abs()) as i128;
    let denominator = b / cancel;
    if denominator.unsigned_abs() != 1 { return Err(arithmetic_error("result requires more fractional digits than the declared scale")); }
    a.checked_mul(factor / cancel).and_then(|v| v.checked_div(denominator))
        .ok_or_else(|| arithmetic_error("numeric overflow"))
}

impl BoundExpression {
    pub(crate) fn evaluate(&self, row: &[Value], target: &Column) -> SqlResult<Value> {
        match self {
            Self::Value(value) => Ok(value.clone()),
            Self::Column(position) => Ok(row[*position].clone()),
            Self::Unary(operator, operand) => {
                let Some(value) = coefficient(operand.evaluate(row, target)?)? else { return Ok(Value::Null); };
                pack(if *operator == '-' { value.checked_neg().ok_or_else(|| arithmetic_error("numeric overflow"))? }
                    else { value }, target)
            },
            Self::Binary(operator, left, right) => {
                let left = coefficient(left.evaluate(row, target)?)?;
                let right = coefficient(right.evaluate(row, target)?)?;
                let (Some(left), Some(right)) = (left, right) else { return Ok(Value::Null); };
                let value = match (operator, &target.data_type) {
                    ('*', DataType::Decimal { scale, .. }) => decimal_multiply(left, right, 10_i128.pow(u32::from(*scale)))?,
                    ('/', DataType::Decimal { scale, .. }) => decimal_divide(left, right, 10_i128.pow(u32::from(*scale)))?,
                    ('+', _) => left.checked_add(right).ok_or_else(|| arithmetic_error("numeric overflow"))?,
                    ('-', _) => left.checked_sub(right).ok_or_else(|| arithmetic_error("numeric overflow"))?,
                    ('*', _) => left.checked_mul(right).ok_or_else(|| arithmetic_error("numeric overflow"))?,
                    ('/', _) if right == 0 => return Err(arithmetic_error("division by zero")),
                    ('/', _) => left.checked_div(right).ok_or_else(|| arithmetic_error("numeric overflow"))?,
                    _ => unreachable!("bound arithmetic operator"),
                };
                pack(value, target)
            },
        }
    }
}
