//! DHIS2 expression language.
//!
//! Supported constructs:
//!
//! - **References**
//!   - `#{DE_UID}` / `#{DE_UID.COC_UID}` / `#{DE_UID.COC_UID.AOC_UID}` — data element value.
//!   - `C{constant_uid}` — constant value (resolved via the catalog).
//!   - `V{variable_name}` — reporting variable (e.g. `event_count`, `enrollment_count`,
//!     `org_unit_count`, `days`, `period_count`, `current_date`).
//!   - `I{indicator_uid}` — nested indicator reference (evaluated by the host).
//!   - `A{attr_uid}` or `A{program_uid.attr_uid}` — tracked-entity attribute (program
//!     indicators).
//!   - `D{program_stage_uid.data_element_uid}` — program-stage data element (program
//!     indicators).
//!   - `[days]` — equivalent to `V{days}`.
//!
//! - **Literals**: integers, decimals, `true`, `false`, single or double quoted strings.
//!
//! - **Operators** (precedence, highest first):
//!   1. unary `+`, `-`, `!`
//!   2. `*`, `/`, `%`
//!   3. `+`, `-`
//!   4. `<`, `<=`, `>`, `>=`
//!   5. `==`, `!=`
//!   6. `&&`
//!   7. `||`
//!
//! - **Whitelisted functions**: `if(cond,then,else)`, `isNull(x)`, `isNotNull(x)`,
//!   `firstNonNull(...)`, `coalesce(...)`, `greatest(...)`, `least(...)`, `abs(x)`,
//!   `mod(a,b)`, `power(base,exp)`, `log(x)` / `log(base,x)`, `log10(x)`, `oizp(x)`
//!   (DHIS2-specific: one if zero or positive, else zero).
//!
//! Unknown function names reject with a precise diagnostic (spec §6.1). The parser does not
//! silently approximate.

use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

/// A dynamically-typed value flowing through an expression evaluation.
///
/// DHIS2 is three-valued: a missing data-element value is `Null` and propagates through
/// arithmetic and comparisons the same way SQL NULL does.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EValue {
    Null,
    Number(f64),
    Bool(bool),
    Str(String),
}

impl EValue {
    pub fn is_null(&self) -> bool {
        matches!(self, EValue::Null)
    }

    pub fn number(n: f64) -> Self {
        EValue::Number(n)
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            EValue::Number(n) => Some(*n),
            EValue::Bool(true) => Some(1.0),
            EValue::Bool(false) => Some(0.0),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            EValue::Bool(b) => Some(*b),
            EValue::Number(n) => Some(*n != 0.0),
            _ => None,
        }
    }
}

/// Reference flavours the engine accepts.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefKind {
    /// `#{DE}` or `#{DE.COC}` or `#{DE.COC.AOC}`
    DataElement {
        de: String,
        coc: Option<String>,
        aoc: Option<String>,
    },
    /// `C{uid}` — constant.
    Constant(String),
    /// `V{name}` — reporting variable.
    Variable(String),
    /// `I{uid}` — nested indicator reference.
    Indicator(String),
    /// `A{uid}` — tracked-entity attribute.
    Attribute {
        program: Option<String>,
        attribute: String,
    },
    /// `D{stage.de}` — program-stage data element.
    StageDataElement { stage: String, de: String },
    /// `[days]` — days in period (sugar for `V{days}`).
    Days,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    // arithmetic
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    // comparison
    Eq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    // logical
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ExpressionAst {
    Null,
    Number(f64),
    Bool(bool),
    String(String),
    Ref(RefKind),
    Unary {
        op: UnaryOp,
        inner: Box<ExpressionAst>,
    },
    Binary {
        op: BinOp,
        lhs: Box<ExpressionAst>,
        rhs: Box<ExpressionAst>,
    },
    Call {
        name: String,
        args: Vec<ExpressionAst>,
    },
}

/// Resolver trait for host-supplied context. Each method returns `None` to indicate NULL.
pub trait Resolver {
    fn reference(&mut self, r: &RefKind) -> DapResult<EValue>;
}

impl<F> Resolver for F
where
    F: FnMut(&RefKind) -> DapResult<EValue>,
{
    fn reference(&mut self, r: &RefKind) -> DapResult<EValue> {
        self(r)
    }
}

/// Legacy flat resolver signature retained for backwards compatibility with earlier Indicator
/// callers that only cared about `#{DE}` / `#{DE.COC}` references.
pub struct DataElementResolver<'a> {
    pub lookup: &'a mut dyn FnMut(&str, Option<&str>) -> Option<f64>,
}

impl<'a> Resolver for DataElementResolver<'a> {
    fn reference(&mut self, r: &RefKind) -> DapResult<EValue> {
        match r {
            RefKind::DataElement { de, coc, .. } => match (self.lookup)(de, coc.as_deref()) {
                Some(n) => Ok(EValue::Number(n)),
                None => Ok(EValue::Null),
            },
            _ => Err(DapError::unsupported(format!(
                "DataElementResolver cannot resolve {r:?}; supply a full Resolver"
            ))),
        }
    }
}

pub fn compile(src: &str) -> DapResult<ExpressionAst> {
    let tokens = tokenize(src)?;
    let mut p = Parser { tokens, pos: 0 };
    let ast = p.parse_expr()?;
    if p.pos != p.tokens.len() {
        return Err(DapError::invalid_value(format!(
            "unexpected trailing input at offset {}",
            p.tokens[p.pos].offset
        )));
    }
    Ok(ast)
}

/// Whitelist of function names the engine implements. Used by `validate_functions` to
/// reject unsupported calls at compile time rather than at evaluation.
pub const SUPPORTED_FUNCTIONS: &[&str] = &[
    "if",
    "isNull",
    "isNotNull",
    "firstNonNull",
    "coalesce",
    "greatest",
    "least",
    "abs",
    "mod",
    "power",
    "log",
    "log10",
    "oizp",
];

/// Walk the AST and ensure every function call targets a supported name. Returns the first
/// unsupported call found. Program indicator + indicator compilers invoke this to fail
/// closed per spec §6.1.
pub fn validate_functions(ast: &ExpressionAst) -> DapResult<()> {
    match ast {
        ExpressionAst::Null
        | ExpressionAst::Number(_)
        | ExpressionAst::Bool(_)
        | ExpressionAst::String(_)
        | ExpressionAst::Ref(_) => Ok(()),
        ExpressionAst::Unary { inner, .. } => validate_functions(inner),
        ExpressionAst::Binary { lhs, rhs, .. } => {
            validate_functions(lhs)?;
            validate_functions(rhs)
        }
        ExpressionAst::Call { name, args } => {
            if !SUPPORTED_FUNCTIONS.contains(&name.as_str()) {
                return Err(DapError::UnsupportedFeature(format!(
                    "function '{name}' is not in the v1 expression grammar; \
                     compile a program indicator that uses it only after the function \
                     matrix lands"
                )));
            }
            for a in args {
                validate_functions(a)?;
            }
            Ok(())
        }
    }
}

/// Legacy entry point: evaluate using a flat `(de, coc) -> Option<f64>` resolver.
pub fn evaluate(
    ast: &ExpressionAst,
    resolver: &mut dyn FnMut(&str, Option<&str>) -> Option<f64>,
) -> DapResult<Option<f64>> {
    let mut adapter = DataElementResolver { lookup: resolver };
    let v = evaluate_expr(ast, &mut adapter)?;
    Ok(v.as_number())
}

/// General evaluator — the preferred entry point. Returns a typed `EValue`.
pub fn evaluate_expr(ast: &ExpressionAst, resolver: &mut dyn Resolver) -> DapResult<EValue> {
    match ast {
        ExpressionAst::Null => Ok(EValue::Null),
        ExpressionAst::Number(n) => Ok(EValue::Number(*n)),
        ExpressionAst::Bool(b) => Ok(EValue::Bool(*b)),
        ExpressionAst::String(s) => Ok(EValue::Str(s.clone())),
        ExpressionAst::Ref(r) => resolver.reference(r),
        ExpressionAst::Unary { op, inner } => {
            let v = evaluate_expr(inner, resolver)?;
            if v.is_null() {
                return Ok(EValue::Null);
            }
            Ok(match op {
                UnaryOp::Neg => match v.as_number() {
                    Some(n) => EValue::Number(-n),
                    None => EValue::Null,
                },
                UnaryOp::Not => match v.as_bool() {
                    Some(b) => EValue::Bool(!b),
                    None => EValue::Null,
                },
            })
        }
        ExpressionAst::Binary { op, lhs, rhs } => eval_binary(op, lhs, rhs, resolver),
        ExpressionAst::Call { name, args } => eval_call(name, args, resolver),
    }
}

fn eval_binary(
    op: &BinOp,
    lhs: &ExpressionAst,
    rhs: &ExpressionAst,
    resolver: &mut dyn Resolver,
) -> DapResult<EValue> {
    // Short-circuit for logicals; null-safe per DHIS2 convention.
    match op {
        BinOp::And => {
            let l = evaluate_expr(lhs, resolver)?;
            if let Some(false) = l.as_bool() {
                return Ok(EValue::Bool(false));
            }
            let r = evaluate_expr(rhs, resolver)?;
            return Ok(match (l.as_bool(), r.as_bool()) {
                (Some(a), Some(b)) => EValue::Bool(a && b),
                _ => EValue::Null,
            });
        }
        BinOp::Or => {
            let l = evaluate_expr(lhs, resolver)?;
            if let Some(true) = l.as_bool() {
                return Ok(EValue::Bool(true));
            }
            let r = evaluate_expr(rhs, resolver)?;
            return Ok(match (l.as_bool(), r.as_bool()) {
                (Some(a), Some(b)) => EValue::Bool(a || b),
                _ => EValue::Null,
            });
        }
        _ => {}
    }

    let l = evaluate_expr(lhs, resolver)?;
    let r = evaluate_expr(rhs, resolver)?;
    if l.is_null() || r.is_null() {
        return Ok(EValue::Null);
    }
    Ok(match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
            let a = l.as_number().ok_or_else(|| {
                DapError::TypeError {
                    expected: "number".into(),
                    found: format!("{l:?}"),
                }
            })?;
            let b = r.as_number().ok_or_else(|| {
                DapError::TypeError {
                    expected: "number".into(),
                    found: format!("{r:?}"),
                }
            })?;
            let v = match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => {
                    if b == 0.0 {
                        return Ok(EValue::Null);
                    }
                    a / b
                }
                BinOp::Mod => {
                    if b == 0.0 {
                        return Ok(EValue::Null);
                    }
                    a % b
                }
                _ => unreachable!(),
            };
            EValue::Number(v)
        }
        BinOp::Eq => EValue::Bool(value_eq(&l, &r)),
        BinOp::NotEq => EValue::Bool(!value_eq(&l, &r)),
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            let ord = value_cmp(&l, &r)?;
            EValue::Bool(match op {
                BinOp::Lt => ord.is_lt(),
                BinOp::LtEq => ord.is_le(),
                BinOp::Gt => ord.is_gt(),
                BinOp::GtEq => ord.is_ge(),
                _ => unreachable!(),
            })
        }
        BinOp::And | BinOp::Or => unreachable!(),
    })
}

fn value_eq(a: &EValue, b: &EValue) -> bool {
    match (a, b) {
        (EValue::Null, _) | (_, EValue::Null) => false,
        (EValue::Number(x), EValue::Number(y)) => x == y,
        (EValue::Bool(x), EValue::Bool(y)) => x == y,
        (EValue::Str(x), EValue::Str(y)) => x == y,
        // Coerce boolean to number where the other side is numeric.
        (EValue::Number(x), EValue::Bool(y)) | (EValue::Bool(y), EValue::Number(x)) => {
            *x == if *y { 1.0 } else { 0.0 }
        }
        _ => false,
    }
}

fn value_cmp(a: &EValue, b: &EValue) -> DapResult<std::cmp::Ordering> {
    match (a, b) {
        (EValue::Number(x), EValue::Number(y)) => Ok(x.total_cmp(y)),
        (EValue::Str(x), EValue::Str(y)) => Ok(x.cmp(y)),
        (EValue::Bool(x), EValue::Bool(y)) => Ok(x.cmp(y)),
        (EValue::Number(x), EValue::Bool(y)) => {
            let yn: f64 = if *y { 1.0 } else { 0.0 };
            Ok(x.total_cmp(&yn))
        }
        (EValue::Bool(x), EValue::Number(y)) => {
            let xn: f64 = if *x { 1.0 } else { 0.0 };
            Ok(xn.total_cmp(y))
        }
        _ => Err(DapError::TypeError {
            expected: "comparable types".into(),
            found: format!("{a:?} vs {b:?}"),
        }),
    }
}

fn eval_call(
    name: &str,
    args: &[ExpressionAst],
    resolver: &mut dyn Resolver,
) -> DapResult<EValue> {
    match name {
        // if(cond, then, else) — null in cond returns null.
        "if" => {
            if args.len() != 3 {
                return Err(DapError::invalid_value(
                    "if(cond, then, else) takes exactly 3 arguments",
                ));
            }
            let cond = evaluate_expr(&args[0], resolver)?;
            match cond.as_bool() {
                Some(true) => evaluate_expr(&args[1], resolver),
                Some(false) => evaluate_expr(&args[2], resolver),
                None => Ok(EValue::Null),
            }
        }
        "isNull" => {
            if args.len() != 1 {
                return Err(DapError::invalid_value("isNull(x) takes 1 argument"));
            }
            let v = evaluate_expr(&args[0], resolver)?;
            Ok(EValue::Bool(v.is_null()))
        }
        "isNotNull" => {
            if args.len() != 1 {
                return Err(DapError::invalid_value("isNotNull(x) takes 1 argument"));
            }
            let v = evaluate_expr(&args[0], resolver)?;
            Ok(EValue::Bool(!v.is_null()))
        }
        "firstNonNull" | "coalesce" => {
            if args.is_empty() {
                return Err(DapError::invalid_value(format!("{name}() needs >= 1 argument")));
            }
            for a in args {
                let v = evaluate_expr(a, resolver)?;
                if !v.is_null() {
                    return Ok(v);
                }
            }
            Ok(EValue::Null)
        }
        "greatest" => reduce_numeric(name, args, resolver, |acc, v| {
            if acc.is_nan() || v > acc {
                v
            } else {
                acc
            }
        }),
        "least" => reduce_numeric(name, args, resolver, |acc, v| {
            if acc.is_nan() || v < acc {
                v
            } else {
                acc
            }
        }),
        "abs" => unary_numeric(name, args, resolver, |x| x.abs()),
        "mod" => {
            if args.len() != 2 {
                return Err(DapError::invalid_value("mod(a, b) takes 2 arguments"));
            }
            let a = numeric_arg(&args[0], resolver)?;
            let b = numeric_arg(&args[1], resolver)?;
            match (a, b) {
                (Some(x), Some(y)) if y != 0.0 => Ok(EValue::Number(x % y)),
                _ => Ok(EValue::Null),
            }
        }
        "power" => {
            if args.len() != 2 {
                return Err(DapError::invalid_value("power(base, exp) takes 2 arguments"));
            }
            match (numeric_arg(&args[0], resolver)?, numeric_arg(&args[1], resolver)?) {
                (Some(x), Some(y)) => Ok(EValue::Number(x.powf(y))),
                _ => Ok(EValue::Null),
            }
        }
        "log" => {
            match args.len() {
                1 => unary_numeric(name, args, resolver, |x| x.ln()),
                2 => {
                    let base = numeric_arg(&args[0], resolver)?;
                    let v = numeric_arg(&args[1], resolver)?;
                    match (base, v) {
                        (Some(b), Some(x)) if b > 0.0 && x > 0.0 => {
                            Ok(EValue::Number(x.log(b)))
                        }
                        _ => Ok(EValue::Null),
                    }
                }
                _ => Err(DapError::invalid_value("log takes 1 or 2 arguments")),
            }
        }
        "log10" => unary_numeric(name, args, resolver, |x| x.log10()),
        // DHIS2-specific: One If Zero or Positive, zero otherwise. Null-in -> null-out.
        "oizp" => {
            if args.len() != 1 {
                return Err(DapError::invalid_value("oizp(x) takes 1 argument"));
            }
            match numeric_arg(&args[0], resolver)? {
                Some(x) if x >= 0.0 => Ok(EValue::Number(1.0)),
                Some(_) => Ok(EValue::Number(0.0)),
                None => Ok(EValue::Null),
            }
        }
        other => Err(DapError::UnsupportedFeature(format!(
            "function '{other}' is not in the v1 expression grammar; \
             compile a program indicator that uses it only after the function \
             matrix lands"
        ))),
    }
}

fn unary_numeric(
    name: &str,
    args: &[ExpressionAst],
    resolver: &mut dyn Resolver,
    f: impl FnOnce(f64) -> f64,
) -> DapResult<EValue> {
    if args.len() != 1 {
        return Err(DapError::invalid_value(format!("{name}(x) takes 1 argument")));
    }
    match numeric_arg(&args[0], resolver)? {
        Some(x) => Ok(EValue::Number(f(x))),
        None => Ok(EValue::Null),
    }
}

fn reduce_numeric(
    name: &str,
    args: &[ExpressionAst],
    resolver: &mut dyn Resolver,
    mut step: impl FnMut(f64, f64) -> f64,
) -> DapResult<EValue> {
    if args.is_empty() {
        return Err(DapError::invalid_value(format!("{name}() needs >= 1 argument")));
    }
    let mut acc = f64::NAN;
    let mut seen = false;
    for a in args {
        if let Some(x) = numeric_arg(a, resolver)? {
            acc = if seen { step(acc, x) } else { x };
            seen = true;
        }
    }
    if seen {
        Ok(EValue::Number(acc))
    } else {
        Ok(EValue::Null)
    }
}

fn numeric_arg(a: &ExpressionAst, resolver: &mut dyn Resolver) -> DapResult<Option<f64>> {
    let v = evaluate_expr(a, resolver)?;
    Ok(v.as_number())
}

// ------------------------------- Lexer -------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Number(f64),
    String(String),
    Ident(String),
    Ref(RefKind),
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    Comma,
    EqEq,
    NotEq,
    Lt,
    LtEq,
    Gt,
    GtEq,
    AndAnd,
    OrOr,
    Not,
    True,
    False,
    Null,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    offset: usize,
}

fn tokenize(src: &str) -> DapResult<Vec<Token>> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        match c {
            '+' => {
                out.push(Token { tok: Tok::Plus, offset: start });
                i += 1;
            }
            '-' => {
                out.push(Token { tok: Tok::Minus, offset: start });
                i += 1;
            }
            '*' => {
                out.push(Token { tok: Tok::Star, offset: start });
                i += 1;
            }
            '/' => {
                out.push(Token { tok: Tok::Slash, offset: start });
                i += 1;
            }
            '%' => {
                out.push(Token { tok: Tok::Percent, offset: start });
                i += 1;
            }
            '(' => {
                out.push(Token { tok: Tok::LParen, offset: start });
                i += 1;
            }
            ')' => {
                out.push(Token { tok: Tok::RParen, offset: start });
                i += 1;
            }
            ',' => {
                out.push(Token { tok: Tok::Comma, offset: start });
                i += 1;
            }
            '=' if bytes.get(i + 1) == Some(&b'=') => {
                out.push(Token { tok: Tok::EqEq, offset: start });
                i += 2;
            }
            '!' if bytes.get(i + 1) == Some(&b'=') => {
                out.push(Token { tok: Tok::NotEq, offset: start });
                i += 2;
            }
            '!' => {
                out.push(Token { tok: Tok::Not, offset: start });
                i += 1;
            }
            '<' if bytes.get(i + 1) == Some(&b'=') => {
                out.push(Token { tok: Tok::LtEq, offset: start });
                i += 2;
            }
            '<' => {
                out.push(Token { tok: Tok::Lt, offset: start });
                i += 1;
            }
            '>' if bytes.get(i + 1) == Some(&b'=') => {
                out.push(Token { tok: Tok::GtEq, offset: start });
                i += 2;
            }
            '>' => {
                out.push(Token { tok: Tok::Gt, offset: start });
                i += 1;
            }
            '&' if bytes.get(i + 1) == Some(&b'&') => {
                out.push(Token { tok: Tok::AndAnd, offset: start });
                i += 2;
            }
            '|' if bytes.get(i + 1) == Some(&b'|') => {
                out.push(Token { tok: Tok::OrOr, offset: start });
                i += 2;
            }
            '0'..='9' | '.' => {
                let mut end = i;
                while end < bytes.len()
                    && (bytes[end].is_ascii_digit() || bytes[end] == b'.')
                {
                    end += 1;
                }
                let s = &src[i..end];
                let value: f64 = s.parse().map_err(|e: std::num::ParseFloatError| {
                    DapError::invalid_value(format!("numeric literal '{s}': {e}"))
                })?;
                out.push(Token {
                    tok: Tok::Number(value),
                    offset: start,
                });
                i = end;
            }
            '\'' | '"' => {
                let quote = c;
                let mut end = i + 1;
                while end < bytes.len() && bytes[end] as char != quote {
                    end += 1;
                }
                if end >= bytes.len() {
                    return Err(DapError::invalid_value(format!(
                        "unterminated string literal at offset {i}"
                    )));
                }
                let s = src[i + 1..end].to_string();
                out.push(Token {
                    tok: Tok::String(s),
                    offset: start,
                });
                i = end + 1;
            }
            '#' | 'C' | 'V' | 'I' | 'A' | 'D'
                if bytes.get(i + 1) == Some(&b'{') =>
            {
                let kind = c;
                let brace_start = i + 2;
                let close = src[brace_start..]
                    .find('}')
                    .ok_or_else(|| DapError::invalid_value(format!(
                        "unterminated '{kind}{{...}}' reference at offset {i}"
                    )))?;
                let inner = &src[brace_start..brace_start + close];
                let r = parse_ref(kind, inner, i)?;
                out.push(Token {
                    tok: Tok::Ref(r),
                    offset: start,
                });
                i = brace_start + close + 1;
            }
            '[' => {
                let close = src[i + 1..]
                    .find(']')
                    .ok_or_else(|| DapError::invalid_value("unterminated '['"))?;
                let name = src[i + 1..i + 1 + close].trim();
                if !name.eq_ignore_ascii_case("days") {
                    return Err(DapError::UnsupportedFeature(format!(
                        "bracket reference '[{name}]' not supported in v1; only [days] is"
                    )));
                }
                out.push(Token {
                    tok: Tok::Ref(RefKind::Days),
                    offset: start,
                });
                i += 1 + close + 1;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let mut end = i;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric()
                        || bytes[end] == b'_'
                        || bytes[end] == b':')
                {
                    end += 1;
                }
                let name = &src[i..end];
                let tok = match name {
                    "true" => Tok::True,
                    "false" => Tok::False,
                    "null" => Tok::Null,
                    other if other.starts_with("d2:") => {
                        return Err(DapError::UnsupportedFeature(format!(
                            "DHIS2 d2: function '{other}' is reserved for a later gate"
                        )))
                    }
                    other => Tok::Ident(other.to_string()),
                };
                out.push(Token { tok, offset: start });
                i = end;
            }
            other => {
                return Err(DapError::invalid_value(format!(
                    "unexpected character {other:?} at offset {i}"
                )));
            }
        }
    }
    Ok(out)
}

fn parse_ref(kind: char, inner: &str, offset: usize) -> DapResult<RefKind> {
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        return Err(DapError::invalid_value(format!(
            "empty reference '{kind}{{}}' at offset {offset}"
        )));
    }
    Ok(match kind {
        '#' => {
            let parts: Vec<&str> = trimmed.split('.').map(|s| s.trim()).collect();
            match parts.len() {
                1 => RefKind::DataElement {
                    de: parts[0].to_string(),
                    coc: None,
                    aoc: None,
                },
                2 => RefKind::DataElement {
                    de: parts[0].to_string(),
                    coc: Some(parts[1].to_string()),
                    aoc: None,
                },
                3 => RefKind::DataElement {
                    de: parts[0].to_string(),
                    coc: Some(parts[1].to_string()),
                    aoc: Some(parts[2].to_string()),
                },
                _ => {
                    return Err(DapError::invalid_value(format!(
                        "'#{{{trimmed}}}' has too many parts (max 3: DE.COC.AOC)"
                    )))
                }
            }
        }
        'C' => RefKind::Constant(trimmed.to_string()),
        'V' => RefKind::Variable(trimmed.to_string()),
        'I' => RefKind::Indicator(trimmed.to_string()),
        'A' => {
            let parts: Vec<&str> = trimmed.split('.').map(|s| s.trim()).collect();
            match parts.len() {
                1 => RefKind::Attribute {
                    program: None,
                    attribute: parts[0].to_string(),
                },
                2 => RefKind::Attribute {
                    program: Some(parts[0].to_string()),
                    attribute: parts[1].to_string(),
                },
                _ => {
                    return Err(DapError::invalid_value(format!(
                        "'A{{{trimmed}}}' has too many parts (max 2: PROGRAM.ATTR)"
                    )))
                }
            }
        }
        'D' => {
            let parts: Vec<&str> = trimmed.split('.').map(|s| s.trim()).collect();
            if parts.len() != 2 {
                return Err(DapError::invalid_value(format!(
                    "'D{{{trimmed}}}' must be STAGE.DATA_ELEMENT"
                )));
            }
            RefKind::StageDataElement {
                stage: parts[0].to_string(),
                de: parts[1].to_string(),
            }
        }
        _ => unreachable!(),
    })
}

// ------------------------------- Parser -------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos).map(|t| &t.tok)
    }

    fn bump(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, want: &Tok) -> bool {
        if self.peek() == Some(want) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn parse_expr(&mut self) -> DapResult<ExpressionAst> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> DapResult<ExpressionAst> {
        let mut lhs = self.parse_and()?;
        while self.eat(&Tok::OrOr) {
            let rhs = self.parse_and()?;
            lhs = ExpressionAst::Binary {
                op: BinOp::Or,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_and(&mut self) -> DapResult<ExpressionAst> {
        let mut lhs = self.parse_eq()?;
        while self.eat(&Tok::AndAnd) {
            let rhs = self.parse_eq()?;
            lhs = ExpressionAst::Binary {
                op: BinOp::And,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_eq(&mut self) -> DapResult<ExpressionAst> {
        let lhs = self.parse_cmp()?;
        let op = match self.peek() {
            Some(Tok::EqEq) => BinOp::Eq,
            Some(Tok::NotEq) => BinOp::NotEq,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.parse_cmp()?;
        Ok(ExpressionAst::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn parse_cmp(&mut self) -> DapResult<ExpressionAst> {
        let lhs = self.parse_add()?;
        let op = match self.peek() {
            Some(Tok::Lt) => BinOp::Lt,
            Some(Tok::LtEq) => BinOp::LtEq,
            Some(Tok::Gt) => BinOp::Gt,
            Some(Tok::GtEq) => BinOp::GtEq,
            _ => return Ok(lhs),
        };
        self.bump();
        let rhs = self.parse_add()?;
        Ok(ExpressionAst::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
        })
    }

    fn parse_add(&mut self) -> DapResult<ExpressionAst> {
        let mut lhs = self.parse_mul()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => BinOp::Add,
                Some(Tok::Minus) => BinOp::Sub,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_mul()?;
            lhs = ExpressionAst::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_mul(&mut self) -> DapResult<ExpressionAst> {
        let mut lhs = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => BinOp::Mul,
                Some(Tok::Slash) => BinOp::Div,
                Some(Tok::Percent) => BinOp::Mod,
                _ => break,
            };
            self.bump();
            let rhs = self.parse_unary()?;
            lhs = ExpressionAst::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> DapResult<ExpressionAst> {
        match self.peek() {
            Some(Tok::Plus) => {
                self.bump();
                self.parse_unary()
            }
            Some(Tok::Minus) => {
                self.bump();
                Ok(ExpressionAst::Unary {
                    op: UnaryOp::Neg,
                    inner: Box::new(self.parse_unary()?),
                })
            }
            Some(Tok::Not) => {
                self.bump();
                Ok(ExpressionAst::Unary {
                    op: UnaryOp::Not,
                    inner: Box::new(self.parse_unary()?),
                })
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> DapResult<ExpressionAst> {
        let tok = self
            .bump()
            .ok_or_else(|| DapError::invalid_value("unexpected end of expression"))?;
        match tok.tok {
            Tok::Number(n) => Ok(ExpressionAst::Number(n)),
            Tok::String(s) => Ok(ExpressionAst::String(s)),
            Tok::True => Ok(ExpressionAst::Bool(true)),
            Tok::False => Ok(ExpressionAst::Bool(false)),
            Tok::Null => Ok(ExpressionAst::Null),
            Tok::Ref(r) => Ok(ExpressionAst::Ref(r)),
            Tok::LParen => {
                let inner = self.parse_expr()?;
                if !self.eat(&Tok::RParen) {
                    return Err(DapError::invalid_value(format!(
                        "expected ')' at offset {}",
                        self.tokens
                            .get(self.pos)
                            .map(|t| t.offset)
                            .unwrap_or(tok.offset)
                    )));
                }
                Ok(inner)
            }
            Tok::Ident(name) => {
                // Function call iff next is '('
                if self.eat(&Tok::LParen) {
                    let mut args = Vec::new();
                    if !self.eat(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr()?);
                            if self.eat(&Tok::Comma) {
                                continue;
                            }
                            if self.eat(&Tok::RParen) {
                                break;
                            }
                            return Err(DapError::invalid_value(
                                "expected ',' or ')' in function argument list",
                            ));
                        }
                    }
                    Ok(ExpressionAst::Call { name, args })
                } else {
                    Err(DapError::invalid_value(format!(
                        "unexpected identifier '{name}' at offset {} (bare identifiers are \
                         not valid outside of a function call; use #{{UID}} / C{{...}} / \
                         V{{...}} for references)",
                        tok.offset
                    )))
                }
            }
            other => Err(DapError::invalid_value(format!(
                "unexpected token {other:?} at offset {}",
                tok.offset
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eval_with(src: &str, mut f: impl FnMut(&RefKind) -> DapResult<EValue>) -> EValue {
        let ast = compile(src).unwrap();
        let mut adapter = |r: &RefKind| f(r);
        evaluate_expr(&ast, &mut adapter).unwrap()
    }

    fn eval_nums(src: &str, values: &[(&str, f64)]) -> EValue {
        let map: ahash::AHashMap<String, f64> =
            values.iter().map(|(k, v)| ((*k).into(), *v)).collect();
        eval_with(src, |r| match r {
            RefKind::DataElement { de, .. } => Ok(map
                .get(de)
                .copied()
                .map(EValue::Number)
                .unwrap_or(EValue::Null)),
            RefKind::Constant(c) => Ok(map
                .get(c.as_str())
                .copied()
                .map(EValue::Number)
                .unwrap_or(EValue::Null)),
            RefKind::Variable(name) => match name.as_str() {
                "days" => Ok(EValue::Number(30.0)),
                _ => Ok(EValue::Null),
            },
            RefKind::Days => Ok(EValue::Number(30.0)),
            _ => Ok(EValue::Null),
        })
    }

    #[test]
    fn full_operator_matrix() {
        assert_eq!(eval_nums("1 + 2 * 3", &[]), EValue::Number(7.0));
        assert_eq!(eval_nums("(1 + 2) * 3", &[]), EValue::Number(9.0));
        assert_eq!(eval_nums("10 % 3", &[]), EValue::Number(1.0));
        assert_eq!(eval_nums("10 / 0", &[]), EValue::Null);
        assert_eq!(eval_nums("1 == 1", &[]), EValue::Bool(true));
        assert_eq!(eval_nums("1 != 2", &[]), EValue::Bool(true));
        assert_eq!(eval_nums("1 < 2 && 3 > 2", &[]), EValue::Bool(true));
        assert_eq!(eval_nums("1 > 2 || 2 >= 2", &[]), EValue::Bool(true));
        assert_eq!(eval_nums("!(1 == 1)", &[]), EValue::Bool(false));
    }

    #[test]
    fn short_circuit_logicals_tolerate_null() {
        assert_eq!(eval_nums("false && #{missing}", &[]), EValue::Bool(false));
        assert_eq!(eval_nums("true || #{missing}", &[]), EValue::Bool(true));
    }

    #[test]
    fn if_expression() {
        let v = eval_nums(
            "if(#{deA000000000} > 0, #{deA000000000}, 0)",
            &[("deA000000000", 5.0)],
        );
        assert_eq!(v, EValue::Number(5.0));
    }

    #[test]
    fn first_non_null_and_coalesce() {
        assert_eq!(
            eval_nums("firstNonNull(#{missing}, #{deAAAAAAAAA}, 0)", &[("deAAAAAAAAA", 7.0)]),
            EValue::Number(7.0)
        );
        assert_eq!(
            eval_nums("coalesce(#{missing1}, #{missing2}, 42)", &[]),
            EValue::Number(42.0)
        );
    }

    #[test]
    fn greatest_least_abs_oizp() {
        assert_eq!(eval_nums("greatest(3, 1, 7, 2)", &[]), EValue::Number(7.0));
        assert_eq!(eval_nums("least(3, 1, 7, 2)", &[]), EValue::Number(1.0));
        assert_eq!(eval_nums("abs(-9.5)", &[]), EValue::Number(9.5));
        assert_eq!(eval_nums("oizp(5)", &[]), EValue::Number(1.0));
        assert_eq!(eval_nums("oizp(0)", &[]), EValue::Number(1.0));
        assert_eq!(eval_nums("oizp(-1)", &[]), EValue::Number(0.0));
    }

    #[test]
    fn log_and_power() {
        let v = eval_nums("power(2, 10)", &[]).as_number().unwrap();
        assert!((v - 1024.0).abs() < 1e-9);
        let v = eval_nums("log(2.71828182845904523)", &[]).as_number().unwrap();
        assert!((v - 1.0).abs() < 1e-5);
        assert_eq!(eval_nums("log10(1000)", &[]).as_number().unwrap().round(), 3.0);
        let v = eval_nums("log(2, 8)", &[]).as_number().unwrap();
        assert!((v - 3.0).abs() < 1e-9);
    }

    #[test]
    fn variables_and_constants() {
        assert_eq!(
            eval_nums("[days] * #{deAAAAAAAAA}", &[("deAAAAAAAAA", 2.0)]),
            EValue::Number(60.0)
        );
        assert_eq!(
            eval_nums("V{days} + C{cPI0000000}", &[("cPI0000000", 2.5)]),
            EValue::Number(32.5)
        );
    }

    #[test]
    fn de_coc_aoc_reference_shapes() {
        let ast = compile("#{deA.cocA.aocA}").unwrap();
        if let ExpressionAst::Ref(RefKind::DataElement { de, coc, aoc }) = ast {
            assert_eq!(de, "deA");
            assert_eq!(coc.as_deref(), Some("cocA"));
            assert_eq!(aoc.as_deref(), Some("aocA"));
        } else {
            panic!("wrong shape");
        }
    }

    #[test]
    fn program_indicator_refs_parse() {
        let a = compile("A{tea000000}").unwrap();
        if let ExpressionAst::Ref(RefKind::Attribute { program, attribute }) = a {
            assert_eq!(program, None);
            assert_eq!(attribute, "tea000000");
        } else {
            panic!("A{{}} did not parse to Attribute");
        }

        let d = compile("D{ps00000000.de00000000}").unwrap();
        if let ExpressionAst::Ref(RefKind::StageDataElement { stage, de }) = d {
            assert_eq!(stage, "ps00000000");
            assert_eq!(de, "de00000000");
        } else {
            panic!("D{{}} did not parse to StageDataElement");
        }
    }

    #[test]
    fn string_literal_and_equality() {
        let v = eval_with("if('yes' == 'yes', 1, 0)", |_| Ok(EValue::Null));
        assert_eq!(v, EValue::Number(1.0));
    }

    #[test]
    fn null_propagation_through_arithmetic() {
        // Any null operand produces null for arithmetic.
        assert_eq!(eval_nums("#{missing} + 1", &[]), EValue::Null);
        assert_eq!(eval_nums("1 - #{missing}", &[]), EValue::Null);
    }

    #[test]
    fn unsupported_function_rejected() {
        let err = compile("stddev(#{de1})").unwrap();
        // Compiler accepts the parse; evaluator rejects the call.
        let mut r = |_: &RefKind| Ok(EValue::Null);
        let e = evaluate_expr(&err, &mut r).expect_err("stddev rejected at eval");
        assert!(e.to_string().contains("stddev"));
    }

    #[test]
    fn d2_namespace_rejected_at_parse() {
        let err = compile("d2:yearsBetween('2020', '2024')").expect_err("d2: rejected");
        assert!(err.to_string().contains("d2:"));
    }

    #[test]
    fn legacy_evaluate_still_works() {
        // The old `evaluate` entry point drives compiled ASTs via a simple (de, coc) -> f64
        // closure; existing dap-dhis2 indicator code paths rely on it.
        let ast = compile("#{deAAAAAAAAA} + #{deBBBBBBBBB}").unwrap();
        let map: ahash::AHashMap<&'static str, f64> =
            [("deAAAAAAAAA", 10.0), ("deBBBBBBBBB", 5.0)].into_iter().collect();
        let mut lookup = |de: &str, _coc: Option<&str>| map.get(de).copied();
        let v = evaluate(&ast, &mut lookup).unwrap();
        assert_eq!(v, Some(15.0));
    }
}
