//! Bradian's first executable, intentionally bounded SQL profile.
//!
//! Parser, binder and executor are native Rust and use the durable G11 catalog
//! and G4 typed WAL-backed tables. Only fully qualified db.schema.table names
//! are supported. DML WHERE predicates must match the entire primary key.
//! No partial execution: INSERT validates all supplied values before touching
//! the table, and UPDATE/DELETE operate on one matching primary key.
//!
//! Unsupported SQL is rejected: this is not a standards-complete parser,
//! general expression evaluator, multi-row transaction engine, or SQL server.

use std::collections::BTreeMap;
use crate::runtime_limits::ScanControl;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use thiserror::Error;

use crate::{Column, DataType, PrimaryKey, Table, TableSpec};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::row_store::BaceWiring;
use crate::typed_table::{TableError, TableStore, TableTransaction, Value};

#[derive(Debug, Error)]
pub enum SqlError {
    #[error(transparent)]
    Resource(#[from] crate::runtime_limits::ResourceError),
    #[error("SQL syntax error: {0}")]
    Syntax(String),
    #[error("SQL unsupported feature: {0}")]
    Unsupported(String),
    #[error("SQL name resolution: {0}")]
    Resolution(String),
    #[error("SQL type error: {0}")]
    Type(String),
    #[error(transparent)]
    Catalog(#[from] PersistentCatalogError),
    #[error(transparent)]
    Table(#[from] TableError),
    #[error(transparent)]
    Domain(#[from] crate::transaction_domain::DomainError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// ADR 0098 — authz denial. Returned by `execute_as` when
    /// the AuthorizationContext lacks the capability required
    /// for the statement.
    #[error(transparent)]
    Authz(#[from] bradian_core::AuthorizationDenied),
    /// ADR 0143 — per-query budget breach. Returned by the
    /// SELECT path when a caller-supplied `BudgetTracker`
    /// reports wall-clock / row / byte / iteration overrun.
    #[error(transparent)]
    Budget(#[from] crate::query_budget::BudgetExceeded),
    /// ADR 0153 — row/column/tenant policy error from
    /// ADR 0123 primitives. The SELECT path translates
    /// unknown-column / null-tenant / non-string-tenant
    /// into this variant.
    #[error(transparent)]
    Policy(#[from] crate::access_policy::PolicyError),
    /// ADR 0174 — audit chain append failed in
    /// `execute_as_audited`. The SQL result itself is
    /// discarded in favor of surfacing the audit failure
    /// so callers never record a silent gap in the chain.
    #[error("audit error: {0}")]
    Audit(String),
    /// ADR 0176 — table-level UNIQUE constraint violated at
    /// INSERT. Carries the offending constraint name and the
    /// columns it covers.
    #[error(transparent)]
    Unique(#[from] crate::unique_check::UniqueViolation),
    /// ADR 0182 — per-column CHECK constraint violated by a
    /// candidate row at INSERT or UPDATE.
    #[error("CHECK constraint on column {column:?} failed: value does not satisfy {op} {literal}")]
    Check { column: String, op: &'static str, literal: String },
    /// ADR 0184 — FOREIGN KEY constraint violated.
    /// `reason` distinguishes the three paths:
    ///   - `missing-parent` — INSERT or UPDATE child FK cols
    ///     reference no parent row.
    ///   - `restrict` — DELETE / PK-UPDATE on parent while a
    ///     child row still references it (RESTRICT default
    ///     action per ADR 0184).
    #[error("FOREIGN KEY constraint {constraint:?} violated: {reason}")]
    ForeignKey { constraint: String, reason: &'static str },
}

pub type SqlResult<T> = Result<T, SqlError>;

/// ADR 0113 — stable identifier for the SQL subset profile
/// this engine implements. Bumped on every corpus extension
/// so clients can detect compat. Each bump has a matching
/// ADR + corpus doc update.
pub const SQL_PROFILE_VERSION: &str = "bradian-sql-g1-2026-11-16";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlResultSet {
    Command { affected: u64 },
    Rows { columns: Vec<String>, rows: Vec<Vec<Value>> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    /// ADR 0181 — `"name"` identifier. Case preserved; never
    /// matched by keyword consume_word/next_is_word paths, so
    /// `"SELECT"` can name a column without colliding with
    /// the reserved word.
    QuotedWord(String),
    Number(String),
    Text(String),
    Symbol(char),
    /// ADR 0186 — JSON path operator `->` (returns a JSON
    /// sub-value).  Lexed as one token so it can't be parsed
    /// as minus-then-gt in the WHERE grammar.
    Arrow,
    /// ADR 0186 — JSON path operator `->>` (returns the
    /// textual form of the sub-value).
    ArrowText,
    /// ADR 0194 — PostgreSQL-style container-path operator
    /// `#>`.  RHS is a text literal `'{a,b,c}'` listing path
    /// segments. Returns the JSON sub-value.
    Hash,
    /// ADR 0194 — container-path operator `#>>`. Same RHS
    /// shape as `#>` but returns the sub-value as text.
    HashText,
}

fn lex(source: &str, expressions: bool) -> SqlResult<Vec<Token>> {
    let mut chars = source.chars().peekable();
    let mut out = Vec::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() { chars.next(); continue; }
        if expressions && ((ch == '-' && chars.clone().nth(1) == Some('-'))
            || (ch == '/' && chars.clone().nth(1) == Some('*'))) {
            return Err(SqlError::Unsupported("SQL comments are unsupported".into()));
        }
        if ch == '\'' {
            chars.next();
            let mut value = String::new();
            loop {
                match chars.next() {
                    Some('\'') if chars.peek() == Some(&'\'') => {
                        chars.next();
                        value.push('\'');
                    }
                    Some('\'') => break,
                    Some(c) => value.push(c),
                    None => return Err(SqlError::Syntax("unterminated string literal".into())),
                }
            }
            out.push(Token::Text(value));
        } else if ch == '"' {
            // ADR 0181 — quoted identifier. `""` inside is an
            // escaped double-quote, mirroring the SQL standard
            // and matching the `''` escape inside a string.
            chars.next();
            let mut value = String::new();
            loop {
                match chars.next() {
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        value.push('"');
                    }
                    Some('"') => break,
                    Some(c) => value.push(c),
                    None => return Err(SqlError::Syntax("unterminated quoted identifier".into())),
                }
            }
            if value.is_empty() {
                return Err(SqlError::Syntax("quoted identifier must not be empty".into()));
            }
            out.push(Token::QuotedWord(value));
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let mut word = String::new();
            while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                word.push(chars.next().unwrap());
            }
            out.push(Token::Word(word));
        } else if ch == '#' && chars.clone().nth(1) == Some('>') {
            // ADR 0194 — `#>` or `#>>` container-path operator.
            chars.next();
            chars.next();
            if chars.peek() == Some(&'>') {
                chars.next();
                out.push(Token::HashText);
            } else {
                out.push(Token::Hash);
            }
        } else if ch == '-' && chars.clone().nth(1) == Some('>') {
            // ADR 0186 — `->` or `->>` JSON path operator.
            // Lexed before the number-prefix branch so the
            // existing "minus must prefix a numeric literal"
            // check still fires for a lone `-`.  ADR 0188 —
            // also emitted in expression mode (HistorySqlEngine
            // uses `Parser::new_expressions`); the expression
            // grammar doesn't bind Arrow so it would surface
            // as a parse error rather than silently lexing as
            // a minus-then-gt pair.
            chars.next();
            chars.next();
            if chars.peek() == Some(&'>') {
                chars.next();
                out.push(Token::ArrowText);
            } else {
                out.push(Token::Arrow);
            }
        } else if ch.is_ascii_digit() || (ch == '-' && !expressions) {
            let mut number = String::new();
            if ch == '-' {
                number.push(chars.next().unwrap());
                if !chars.peek().is_some_and(char::is_ascii_digit) {
                    return Err(SqlError::Syntax("minus must prefix a numeric literal".into()));
                }
            }
            while chars.peek().is_some_and(char::is_ascii_digit) {
                number.push(chars.next().unwrap());
            }
            if chars.peek() == Some(&'.') {
                chars.next();
                number.push('.');
                if !chars.peek().is_some_and(char::is_ascii_digit) {
                    return Err(SqlError::Syntax("decimal requires digits after the point".into()));
                }
                while chars.peek().is_some_and(char::is_ascii_digit) {
                    number.push(chars.next().unwrap());
                }
            }
            out.push(Token::Number(number));
        } else if "(),.=;*<>:".contains(ch) || (expressions && "+-/".contains(ch)) {
            // ADR 0197 — `:` admitted for JSON slice indices
            // (`col -> 0:3`).  It is otherwise never consumed
            // by SQL grammar at this parser level; named-
            // parameter `:name` substitution runs at the
            // driver layer before the SQL reaches the lexer
            // (ADR 0183), so this does not disturb that path.
            // ADR 0182 — `<` and `>` admitted so the CHECK
            // grammar can parse comparison operators. The
            // existing grammar paths reject these symbols
            // outside CHECK (e.g. `rejects_join_aggregate_and_or_and_range_and_alias`
            // continues to reject range predicates in WHERE
            // because the WHERE parser's `predicate` only
            // consumes `=`).
            chars.next();
            out.push(Token::Symbol(ch));
        } else {
            return Err(SqlError::Unsupported(format!("unsupported character '{ch}'")));
        }
        if out.len() > 8192 {
            return Err(SqlError::Unsupported("statement exceeds token budget".into()));
        }
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub(crate) enum Literal {
    Default,
    Null,
    Bool(bool),
    Number(String),
    Text(String),
}

pub(crate) struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub(crate) fn new(sql: &str) -> SqlResult<Self> {
        Ok(Self { tokens: lex(sql, false)?, pos: 0 })
    }

    pub(crate) fn new_expressions(sql: &str) -> SqlResult<Self> {
        Ok(Self { tokens: lex(sql, true)?, pos: 0 })
    }

    pub(crate) fn next_is_literal(&self) -> bool {
        matches!(self.peek(), Some(Token::Text(_) | Token::Number(_)))
            || ["null", "true", "false"].iter().any(|word| self.next_is_word(word))
            || (self.peek() == Some(&Token::Symbol('-'))
                && matches!(self.tokens.get(self.pos + 1), Some(Token::Number(_))))
    }

    fn peek(&self) -> Option<&Token> { self.tokens.get(self.pos) }

    /// ADR 0186 — consume the current token without inspection.
    /// Used when a caller has already matched the token via
    /// `peek` and wants to move the cursor forward.
    pub(crate) fn advance(&mut self) { self.pos += 1; }

    /// ADR 0193 / ADR 0194 — predicate for module-external
    /// callers (`sql_expression`) that need to tell apart an
    /// arithmetic `col = expr` SET element from a JSON-path
    /// mutation (`col -> ... = lit` or `col #> '{…}' = lit`)
    /// without reaching into the private `Token` enum.
    pub(crate) fn next_is_json_arrow(&self) -> bool {
        matches!(self.peek(),
            Some(Token::Arrow) | Some(Token::ArrowText)
            | Some(Token::Hash) | Some(Token::HashText))
    }

    /// ADR 0190 — save / restore the cursor for lookahead.
    pub(crate) fn save_pos(&self) -> usize { self.pos }
    pub(crate) fn restore_pos(&mut self, pos: usize) { self.pos = pos; }

    pub(crate) fn word(&mut self) -> SqlResult<String> {
        match self.tokens.get(self.pos).cloned() {
            Some(Token::Word(word)) => {
                self.pos += 1;
                Ok(word.to_ascii_lowercase())
            }
            // ADR 0181 — quoted identifier. The lexer preserves
            // case in the token so keyword matching can skip
            // quoted forms, but at the identifier boundary we
            // fold to lowercase to match the catalog's ADR 0006
            // case-insensitive storage rule. The practical value
            // of quoting is therefore to let reserved words name
            // user objects, not to introduce case-sensitive
            // identifiers.
            Some(Token::QuotedWord(word)) => {
                self.pos += 1;
                Ok(word.to_ascii_lowercase())
            }
            _ => Err(SqlError::Syntax("expected an identifier".into())),
        }
    }

    pub(crate) fn next_is_word(&self, value: &str) -> bool {
        matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(value))
    }

    pub(crate) fn consume_word(&mut self, value: &str) -> bool {
        if matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(value)) {
            self.pos += 1;
            true
        } else { false }
    }

    pub(crate) fn expect(&mut self, value: &str) -> SqlResult<()> {
        if self.consume_word(value) { Ok(()) }
        else { Err(SqlError::Syntax(format!("expected {value}"))) }
    }

    pub(crate) fn consume_symbol(&mut self, value: char) -> bool {
        if self.peek() == Some(&Token::Symbol(value)) {
            self.pos += 1;
            true
        } else { false }
    }

    pub(crate) fn symbol(&mut self, value: char) -> SqlResult<()> {
        if self.consume_symbol(value) { Ok(()) }
        else { Err(SqlError::Syntax(format!("expected '{value}'"))) }
    }

    /// ADR 0182 — parse a comparison operator for the CHECK
    /// clause. Accepts `=`, `<>`, `<`, `<=`, `>`, `>=`.
    pub(crate) fn word_or_symbol_pair_for_check(&mut self) -> SqlResult<crate::model::CheckOp> {
        use crate::model::CheckOp;
        match self.peek().cloned() {
            Some(Token::Symbol('=')) => { self.pos += 1; Ok(CheckOp::Eq) }
            Some(Token::Symbol('<')) => {
                self.pos += 1;
                if self.consume_symbol('=') { Ok(CheckOp::Le) }
                else if self.consume_symbol('>') { Ok(CheckOp::Ne) }
                else { Ok(CheckOp::Lt) }
            }
            Some(Token::Symbol('>')) => {
                self.pos += 1;
                if self.consume_symbol('=') { Ok(CheckOp::Ge) }
                else { Ok(CheckOp::Gt) }
            }
            _ => Err(SqlError::Syntax("expected CHECK operator (=, <>, <, <=, >, >=)".into())),
        }
    }

    pub(crate) fn name(&mut self) -> SqlResult<Vec<String>> {
        let mut parts = vec![self.word()?];
        while self.consume_symbol('.') {
            parts.push(self.word()?);
            if parts.len() > 3 {
                return Err(SqlError::Unsupported("identifiers have at most three parts".into()));
            }
        }
        Ok(parts)
    }

    pub(crate) fn list<T>(&mut self, mut parse: impl FnMut(&mut Self) -> SqlResult<T>) -> SqlResult<Vec<T>> {
        self.symbol('(')?;
        let mut values = Vec::new();
        loop {
            values.push(parse(self)?);
            if !self.consume_symbol(',') { break; }
        }
        self.symbol(')')?;
        Ok(values)
    }

    pub(crate) fn literal(&mut self) -> SqlResult<Literal> {
        if self.consume_symbol('-') {
            return match self.tokens.get(self.pos).cloned() {
                Some(Token::Number(value)) => {
                    self.pos += 1;
                    Ok(Literal::Number(format!("-{value}")))
                },
                _ => Err(SqlError::Syntax("minus must prefix a numeric literal".into())),
            };
        }
        match self.tokens.get(self.pos).cloned() {
            Some(Token::Text(value)) => { self.pos += 1; Ok(Literal::Text(value)) }
            Some(Token::Number(value)) => { self.pos += 1; Ok(Literal::Number(value)) }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("null") => {
                self.pos += 1; Ok(Literal::Null)
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("true") => {
                self.pos += 1; Ok(Literal::Bool(true))
            }
            Some(Token::Word(word)) if word.eq_ignore_ascii_case("false") => {
                self.pos += 1; Ok(Literal::Bool(false))
            }
            _ => Err(SqlError::Syntax("expected a literal value".into())),
        }
    }

    /// DEFAULT is available only in the historical INSERT profile.
    pub(crate) fn insert_literal(&mut self) -> SqlResult<Literal> {
        if self.consume_word("default") { Ok(Literal::Default) } else { self.literal() }
    }

    /// Parse a nonnegative integer WAL position without lossy casts.
    pub(crate) fn number_u64(&mut self) -> SqlResult<u64> {
        match self.tokens.get(self.pos).cloned() {
            Some(Token::Number(value)) if !value.contains('.') && !value.starts_with('-') => {
                self.pos += 1;
                value.parse().map_err(|_| SqlError::Syntax("invalid WAL LSN".into()))
            }
            _ => Err(SqlError::Syntax("expected unsigned WAL LSN".into())),
        }
    }

    fn number_u32(&mut self) -> SqlResult<u32> {
        match self.tokens.get(self.pos).cloned() {
            Some(Token::Number(value)) if !value.contains('.') && !value.starts_with('-') => {
                self.pos += 1;
                value.parse().map_err(|_| SqlError::Syntax("invalid positive integer".into()))
            }
            _ => Err(SqlError::Syntax("expected nonnegative integer".into())),
        }
    }

    pub(crate) fn data_type(&mut self) -> SqlResult<DataType> {
        let name = self.word()?;
        match name.as_str() {
            "int" | "integer" | "int4" => Ok(DataType::Int32),
            "bigint" | "int8" => Ok(DataType::Int64),
            "boolean" | "bool" => Ok(DataType::Boolean),
            "text" => Ok(DataType::Text),
            "varchar" => {
                self.symbol('(')?;
                let n = self.number_u32()?;
                self.symbol(')')?;
                Ok(DataType::Varchar { max_length: n })
            }
            "decimal" | "numeric" => {
                self.symbol('(')?;
                let precision = self.number_u32()?;
                self.symbol(',')?;
                let scale = self.number_u32()?;
                self.symbol(')')?;
                let precision = u8::try_from(precision)
                    .map_err(|_| SqlError::Type("decimal precision out of range".into()))?;
                let scale = u8::try_from(scale)
                    .map_err(|_| SqlError::Type("decimal scale out of range".into()))?;
                Ok(DataType::Decimal { precision, scale })
            }
            // ADR 0113 — JSON / TimestampTz / Uuid are now
            // reachable from SQL DDL. Values flow through the
            // typed-table engine unchanged; no extra parser
            // work at INSERT time (literal_to_value handles).
            "json" => Ok(DataType::Json),
            "timestamptz" | "timestamp" => Ok(DataType::TimestampTz),
            "uuid" => Ok(DataType::Uuid),
            _ => Err(SqlError::Unsupported(format!("type {name} is not in the G12 SQL profile"))),
        }
    }

    pub(crate) fn assignments(&mut self) -> SqlResult<Vec<(String, Literal)>> {
        let mut fields = Vec::new();
        loop {
            let column = self.word()?;
            self.symbol('=')?;
            fields.push((column, self.literal()?));
            if !self.consume_symbol(',') { break; }
        }
        Ok(fields)
    }

    /// ADR 0192 — SET grammar that also accepts JSON path
    /// assignment LHS: `col = lit`, `col -> seg = lit`, or
    /// `col ->> seg = lit`. Reuses `parse_projection_tail` for
    /// the path-chain portion so array index, chained segments
    /// and `->>` termination all behave the same way as SELECT.
    pub(crate) fn path_assignments(&mut self) -> SqlResult<Vec<SetAssignment>> {
        let mut fields = Vec::new();
        loop {
            let column = self.word()?;
            let next_is_arrow = matches!(self.peek(),
                Some(Token::Arrow) | Some(Token::ArrowText)
                | Some(Token::Hash) | Some(Token::HashText));
            let assignment = if next_is_arrow {
                let projection = parse_projection_tail(self, column)?;
                let (col, path, as_text) = match projection {
                    Projection::JsonPath { column, path, as_text, .. } =>
                        (column, path, as_text),
                    Projection::Column(_) => unreachable!("arrow guard"),
                };
                // ADR 0200 — `REMOVE` keyword after a JSON path
                // deletes the terminal element instead of
                // assigning. ADR 0201 — `INSERT` keyword
                // splices the literal into an array at the
                // terminal index.
                if self.consume_word("remove") {
                    SetAssignment::JsonRemove { column: col, path }
                } else if self.consume_word("insert") {
                    let literal = self.literal()?;
                    SetAssignment::JsonInsert { column: col, path, as_text, value: literal }
                } else if self.consume_word("concat") {
                    let literal = self.literal()?;
                    SetAssignment::JsonConcat { column: col, path, value: literal }
                } else {
                    self.symbol('=')?;
                    let literal = self.literal()?;
                    SetAssignment::JsonPath { column: col, path, as_text, value: literal }
                }
            } else {
                self.symbol('=')?;
                SetAssignment::Column { column, value: self.literal()? }
            };
            fields.push(assignment);
            if !self.consume_symbol(',') { break; }
        }
        Ok(fields)
    }

    fn predicate(&mut self) -> SqlResult<Vec<(String, Literal)>> {
        self.expect("where")?;
        let mut tests = Vec::new();
        loop {
            let column = self.word()?;
            self.symbol('=')?;
            tests.push((column, self.literal()?));
            if !self.consume_word("and") { break; }
        }
        Ok(tests)
    }

    pub(crate) fn end(&mut self) -> SqlResult<()> {
        self.consume_symbol(';');
        if self.pos == self.tokens.len() { Ok(()) }
        else { Err(SqlError::Unsupported("trailing tokens or multiple statements".into())) }
    }
}

pub(crate) fn qualified(parts: &[String], expected: usize) -> SqlResult<()> {
    if parts.len() == expected { Ok(()) }
    else { Err(SqlError::Resolution(format!("expected {expected}-part qualified name"))) }
}

/// ADR 0180 — parse `YYYY-MM-DDTHH:MM:SS[.ffffff][Z|±HH:MM]` into
/// microseconds since the Unix epoch. Returns `None` for any
/// syntax deviation (unknown separator, out-of-range field,
/// missing offset). Uses Howard Hinnant's days-from-civil
/// algorithm to compute the date part without an external
/// dependency.
pub(crate) fn parse_iso8601_micros(s: &str) -> Option<i64> {
    let bytes = s.as_bytes();
    // Minimum: "YYYY-MM-DDTHH:MM:SSZ" = 20 chars.
    if bytes.len() < 20 { return None; }
    let digit = |i: usize| -> Option<i64> {
        let c = *bytes.get(i)?;
        if c.is_ascii_digit() { Some((c - b'0') as i64) } else { None }
    };
    if bytes[4] != b'-' || bytes[7] != b'-'
        || (bytes[10] != b'T' && bytes[10] != b' ')
        || bytes[13] != b':' || bytes[16] != b':'
    { return None; }
    let year = digit(0)? * 1000 + digit(1)? * 100 + digit(2)? * 10 + digit(3)?;
    let month = digit(5)? * 10 + digit(6)?;
    let day = digit(8)? * 10 + digit(9)?;
    let hour = digit(11)? * 10 + digit(12)?;
    let minute = digit(14)? * 10 + digit(15)?;
    let second = digit(17)? * 10 + digit(18)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day)
        || hour > 23 || minute > 59 || second > 60  // leap second permitted
    { return None; }

    let mut pos = 19;
    // Optional fractional seconds: .ffffff (up to 6 digits).
    let mut micros_frac: i64 = 0;
    if bytes.get(pos) == Some(&b'.') {
        pos += 1;
        let mut count = 0;
        while count < 6 && pos < bytes.len() && bytes[pos].is_ascii_digit() {
            micros_frac = micros_frac * 10 + (bytes[pos] - b'0') as i64;
            pos += 1; count += 1;
        }
        // Pad to microsecond precision if fewer digits were given.
        for _ in count..6 { micros_frac *= 10; }
        // Reject extra fractional digits past microsecond precision.
        if pos < bytes.len() && bytes[pos].is_ascii_digit() { return None; }
    }

    // Timezone designator: Z or ±HH:MM.
    let (offset_sec, consumed) = match bytes.get(pos) {
        Some(&b'Z') => (0_i64, 1),
        Some(&b'+') | Some(&b'-') => {
            if bytes.len() < pos + 6 || bytes[pos + 3] != b':' { return None; }
            let sign = if bytes[pos] == b'+' { 1_i64 } else { -1_i64 };
            let hh = digit(pos + 1)? * 10 + digit(pos + 2)?;
            let mm = digit(pos + 4)? * 10 + digit(pos + 5)?;
            if hh > 23 || mm > 59 { return None; }
            (sign * (hh * 3600 + mm * 60), 6)
        }
        _ => return None,
    };
    pos += consumed;
    if pos != bytes.len() { return None; }

    // Howard Hinnant's days_from_civil, adjusted for a 1970-01-01
    // epoch. `m` is the Hinnant-offset month index (March = 0,
    // ..., January = 10, February = 11) so the `doy` formula uses
    // `m` directly rather than `m - 3`.
    let (y, m) = if month <= 2 { (year - 1, month + 9) } else { (year, month - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days_since_epoch = era * 146097 + doe - 719468;

    let secs = days_since_epoch
        .checked_mul(86400)?
        .checked_add(hour * 3600 + minute * 60 + second)?
        .checked_sub(offset_sec)?;
    secs.checked_mul(1_000_000)?.checked_add(micros_frac)
}

/// ADR 0192 + ADR 0200 — SET assignment element: a plain
/// column replacement, a JSON path mutation, or a JSON path
/// remove (REMOVE keyword).
#[derive(Clone, Debug)]
pub(crate) enum SetAssignment {
    Column { column: String, value: Literal },
    JsonPath {
        column: String,
        path: Vec<JsonSegment>,
        as_text: bool,
        value: Literal,
    },
    /// ADR 0200 — delete the terminal JSON element at `path`.
    /// For an array index the surrounding array shifts left;
    /// for an object key the key is dropped from the object.
    /// `Slice` and mid-chain `IndexFromEnd` segments are
    /// rejected at mutation time.
    JsonRemove {
        column: String,
        path: Vec<JsonSegment>,
    },
    /// ADR 0201 — splice: insert `value` at the terminal
    /// array index, shifting subsequent elements right.
    /// `as_text` writes the leaf as a JSON string. Only
    /// positive `Index(n)` with `0 <= n <= arr.len()` is
    /// accepted (`n == len` appends, matching ADR 0198).
    /// `IndexFromEnd`, `Slice`, and object-key terminal
    /// segments are rejected.
    JsonInsert {
        column: String,
        path: Vec<JsonSegment>,
        as_text: bool,
        value: Literal,
    },
    /// ADR 0203 — append every element of a JSON-array
    /// literal onto the terminal array at `path`. `value` must
    /// parse as a JSON array; the terminal of `path` must
    /// resolve to an array in the stored row.
    JsonConcat {
        column: String,
        path: Vec<JsonSegment>,
        value: Literal,
    },
}

/// ADR 0192 — apply a JSON path mutation to the supplied JSON
/// cell, returning the serialized result. Walks the path
/// creating missing intermediate objects along the way; a
/// missing intermediate **array** step cannot be auto-created
/// and surfaces as `SqlError::Type`.
pub(crate) fn apply_json_path_mutation(
    column: &str,
    cell: &Value,
    path: &[JsonSegment],
    as_text: bool,
    value: &Literal,
) -> SqlResult<Value> {
    let source = match cell {
        Value::Null => "null".to_string(),
        Value::Json(s) | Value::String(s) => s.clone(),
        other => return Err(SqlError::Type(format!(
            "JSON mutation requires a JSON cell; column {column} holds {other:?}"))),
    };
    let mut root: serde_json::Value = serde_json::from_str(&source)
        .map_err(|e| SqlError::Type(format!("JSON mutation source does not parse: {e}")))?;
    // If the source is a bare null and the first step is a key,
    // treat the root as an empty object so the assignment
    // creates one.
    if matches!(root, serde_json::Value::Null)
        && matches!(path.first(), Some(JsonSegment::Key(_)))
    {
        root = serde_json::Value::Object(serde_json::Map::new());
    }
    let new_leaf = literal_to_json(value, as_text)?;
    descend_and_assign(&mut root, path, new_leaf)?;
    Ok(Value::Json(serde_json::to_string(&root)
        .map_err(|e| SqlError::Type(format!("JSON mutation serialize: {e}")))?))
}

/// ADR 0201 — insert `value` into an array at the terminal
/// array-index segment of `path`, shifting subsequent
/// elements right. `0 <= n <= len` (where `n == len` appends,
/// consistent with ADR 0198). Non-array parents and non-Index
/// terminal segments yield an error.
pub(crate) fn apply_json_path_insert(
    column: &str,
    cell: &Value,
    path: &[JsonSegment],
    as_text: bool,
    value: &Literal,
) -> SqlResult<Value> {
    if path.is_empty() {
        return Err(SqlError::Unsupported(
            "INSERT requires a JSON path with at least one segment".into()));
    }
    let source = match cell {
        Value::Null => return Err(SqlError::Type(format!(
            "JSON insert on NULL column {column}: source must be an array"))),
        Value::Json(s) | Value::String(s) => s.clone(),
        other => return Err(SqlError::Type(format!(
            "JSON insert requires a JSON cell; column {column} holds {other:?}"))),
    };
    let mut root: serde_json::Value = serde_json::from_str(&source)
        .map_err(|e| SqlError::Type(format!("JSON insert source does not parse: {e}")))?;
    let new_leaf = literal_to_json(value, as_text)?;
    descend_and_insert(&mut root, path, new_leaf)?;
    Ok(Value::Json(serde_json::to_string(&root)
        .map_err(|e| SqlError::Type(format!("JSON insert serialize: {e}")))?))
}

fn descend_and_insert(
    current: &mut serde_json::Value,
    path: &[JsonSegment],
    new_leaf: serde_json::Value,
) -> SqlResult<()> {
    let (seg, rest) = path.split_first().expect("non-empty path");
    if rest.is_empty() {
        match seg {
            JsonSegment::Index(i) => {
                let arr = current.as_array_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON insert: terminal index requires an array".into()))?;
                if *i > arr.len() {
                    return Err(SqlError::Type(format!(
                        "JSON insert: index {i} out of range (len {})", arr.len())));
                }
                arr.insert(*i, new_leaf);
                Ok(())
            }
            _ => Err(SqlError::Unsupported(
                "JSON INSERT terminal segment must be a non-negative integer index".into())),
        }
    } else {
        match seg {
            JsonSegment::Key(k) => {
                let obj = current.as_object_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON insert: intermediate key requires an object".into()))?;
                let next = obj.entry(k.as_str().to_string())
                    .or_insert(serde_json::Value::Null);
                descend_and_insert(next, rest, new_leaf)
            }
            JsonSegment::Index(_) | JsonSegment::IndexFromEnd(_) => {
                let arr = current.as_array_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON insert: intermediate index requires an array".into()))?;
                let len = arr.len();
                let idx = seg.resolve_for_array(len)
                    .ok_or_else(|| SqlError::Type(format!(
                        "JSON insert: intermediate index {seg:?} out of bounds (len {len})")))?;
                descend_and_insert(&mut arr[idx], rest, new_leaf)
            }
            JsonSegment::Slice { .. } => Err(SqlError::Unsupported(
                "JSON slice segments cannot appear on an INSERT path".into())),
        }
    }
}

/// ADR 0203 — append every element of a JSON-array literal onto
/// the array reached by `path`. The literal must itself parse
/// as a JSON array; the terminal of `path` must point at an
/// array in the stored cell. An empty path means the column
/// itself is the array. A `Slice` segment anywhere is rejected.
pub(crate) fn apply_json_path_concat(
    column: &str,
    cell: &Value,
    path: &[JsonSegment],
    value: &Literal,
) -> SqlResult<Value> {
    let addition_raw = match value {
        Literal::Text(s) | Literal::Number(s) => s.clone(),
        _ => return Err(SqlError::Type(
            "JSON CONCAT requires a JSON-array literal (single-quoted text)".into())),
    };
    let addition: serde_json::Value = serde_json::from_str(&addition_raw)
        .map_err(|e| SqlError::Type(format!("JSON CONCAT RHS does not parse: {e}")))?;
    let addition = match addition {
        serde_json::Value::Array(items) => items,
        _ => return Err(SqlError::Type(
            "JSON CONCAT RHS must be a JSON array".into())),
    };
    let source = match cell {
        Value::Null => return Err(SqlError::Type(format!(
            "JSON concat on NULL column {column}: source must be an array"))),
        Value::Json(s) | Value::String(s) => s.clone(),
        other => return Err(SqlError::Type(format!(
            "JSON concat requires a JSON cell; column {column} holds {other:?}"))),
    };
    let mut root: serde_json::Value = serde_json::from_str(&source)
        .map_err(|e| SqlError::Type(format!("JSON concat source does not parse: {e}")))?;
    descend_and_concat(&mut root, path, addition)?;
    Ok(Value::Json(serde_json::to_string(&root)
        .map_err(|e| SqlError::Type(format!("JSON concat serialize: {e}")))?))
}

fn descend_and_concat(
    current: &mut serde_json::Value,
    path: &[JsonSegment],
    addition: Vec<serde_json::Value>,
) -> SqlResult<()> {
    if path.is_empty() {
        let arr = current.as_array_mut()
            .ok_or_else(|| SqlError::Type(
                "JSON concat: target must be an array".into()))?;
        arr.extend(addition);
        return Ok(());
    }
    let (seg, rest) = path.split_first().expect("non-empty path");
    match seg {
        JsonSegment::Key(k) => {
            let obj = current.as_object_mut()
                .ok_or_else(|| SqlError::Type(
                    "JSON concat: intermediate key requires an object".into()))?;
            let next = obj.get_mut(k.as_str())
                .ok_or_else(|| SqlError::Type(format!(
                    "JSON concat: key {k:?} not found")))?;
            descend_and_concat(next, rest, addition)
        }
        JsonSegment::Index(_) | JsonSegment::IndexFromEnd(_) => {
            let arr = current.as_array_mut()
                .ok_or_else(|| SqlError::Type(
                    "JSON concat: intermediate index requires an array".into()))?;
            let len = arr.len();
            let idx = seg.resolve_for_array(len)
                .ok_or_else(|| SqlError::Type(format!(
                    "JSON concat: intermediate index out of bounds (len {len})")))?;
            descend_and_concat(&mut arr[idx], rest, addition)
        }
        JsonSegment::Slice { .. } => Err(SqlError::Unsupported(
            "JSON slice segments cannot appear on a CONCAT path".into())),
    }
}

/// ADR 0200 — delete the terminal element reached by `path`
/// from the JSON cell. Array elements shift left; object
/// keys are dropped. Non-array intermediate indices and
/// `Slice` segments fail with `SqlError::Type` /
/// `SqlError::Unsupported`. An empty path or a terminal
/// `Slice` are rejected.
pub(crate) fn apply_json_path_remove(
    column: &str,
    cell: &Value,
    path: &[JsonSegment],
) -> SqlResult<Value> {
    if path.is_empty() {
        return Err(SqlError::Unsupported(
            "REMOVE requires a JSON path with at least one segment".into()));
    }
    let source = match cell {
        Value::Null => return Ok(Value::Null),
        Value::Json(s) | Value::String(s) => s.clone(),
        other => return Err(SqlError::Type(format!(
            "JSON remove requires a JSON cell; column {column} holds {other:?}"))),
    };
    let mut root: serde_json::Value = serde_json::from_str(&source)
        .map_err(|e| SqlError::Type(format!("JSON remove source does not parse: {e}")))?;
    descend_and_remove(&mut root, path)?;
    Ok(Value::Json(serde_json::to_string(&root)
        .map_err(|e| SqlError::Type(format!("JSON remove serialize: {e}")))?))
}

fn descend_and_remove(
    current: &mut serde_json::Value,
    path: &[JsonSegment],
) -> SqlResult<()> {
    let (seg, rest) = path.split_first().expect("non-empty path");
    if rest.is_empty() {
        // Terminal segment — perform the remove.
        match seg {
            JsonSegment::Key(k) => {
                let obj = current.as_object_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON remove: terminal key segment requires an object".into()))?;
                // Missing key is a no-op (idempotent).
                obj.remove(k.as_str());
                Ok(())
            }
            JsonSegment::Index(_) | JsonSegment::IndexFromEnd(_) => {
                let arr = current.as_array_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON remove: terminal index segment requires an array".into()))?;
                let len = arr.len();
                let Some(idx) = seg.resolve_for_array(len) else {
                    return Err(SqlError::Type(format!(
                        "JSON remove: index {seg:?} out of bounds (len {len})")));
                };
                arr.remove(idx);
                Ok(())
            }
            JsonSegment::Slice { .. } => Err(SqlError::Unsupported(
                "JSON slice segments cannot appear as a REMOVE target".into())),
        }
    } else {
        // Intermediate — descend the usual way.
        match seg {
            JsonSegment::Key(k) => {
                let obj = current.as_object_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON remove: intermediate key requires an object".into()))?;
                match obj.get_mut(k.as_str()) {
                    Some(next) => descend_and_remove(next, rest),
                    // Missing intermediate → nothing to remove; no-op.
                    None => Ok(()),
                }
            }
            JsonSegment::Index(_) | JsonSegment::IndexFromEnd(_) => {
                let arr = current.as_array_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON remove: intermediate index requires an array".into()))?;
                let len = arr.len();
                let Some(idx) = seg.resolve_for_array(len) else {
                    // Out-of-bounds intermediate → nothing to
                    // remove; no-op (same as missing key).
                    return Ok(());
                };
                descend_and_remove(&mut arr[idx], rest)
            }
            JsonSegment::Slice { .. } => Err(SqlError::Unsupported(
                "JSON slice segments cannot appear on a REMOVE path".into())),
        }
    }
}

fn descend_and_assign(
    current: &mut serde_json::Value,
    path: &[JsonSegment],
    new_leaf: serde_json::Value,
) -> SqlResult<()> {
    match path.split_first() {
        None => { *current = new_leaf; Ok(()) }
        Some((seg, rest)) => match seg {
            JsonSegment::Key(k) => {
                if !current.is_object() {
                    // Replacing a non-object intermediate with
                    // an empty object is the SQL-standard
                    // "assign creates container" behavior.
                    *current = serde_json::Value::Object(serde_json::Map::new());
                }
                let obj = current.as_object_mut().expect("just replaced");
                let next = obj.entry(k.as_str().to_string())
                    .or_insert(serde_json::Value::Null);
                if rest.iter().next().is_some()
                    && matches!(next, serde_json::Value::Null)
                    && matches!(rest.first(), Some(JsonSegment::Key(_)))
                {
                    *next = serde_json::Value::Object(serde_json::Map::new());
                }
                descend_and_assign(next, rest, new_leaf)
            }
            JsonSegment::Index(i) if rest.is_empty() => {
                // ADR 0198 — terminal `Index(len)` appends.
                // `Index(> len)` still rejects to preserve the
                // "no sparse arrays" invariant.
                let arr = current.as_array_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON mutation: cannot apply array index to non-array".into()))?;
                let len = arr.len();
                if *i == len {
                    arr.push(new_leaf);
                    Ok(())
                } else if *i < len {
                    arr[*i] = new_leaf;
                    Ok(())
                } else {
                    Err(SqlError::Type(format!(
                        "JSON mutation: array index {i} out of bounds (len {len})")))
                }
            }
            JsonSegment::Index(_) | JsonSegment::IndexFromEnd(_) => {
                let arr = current.as_array_mut()
                    .ok_or_else(|| SqlError::Type(
                        "JSON mutation: cannot apply array index to non-array".into()))?;
                let len = arr.len();
                let idx = seg.resolve_for_array(len)
                    .ok_or_else(|| SqlError::Type(format!(
                        "JSON mutation: array index {seg:?} out of bounds (len {len})")))?;
                descend_and_assign(&mut arr[idx], rest, new_leaf)
            }
            JsonSegment::Slice { .. } => Err(SqlError::Unsupported(
                "JSON slice segments cannot appear on an UPDATE SET left-hand side".into())),
        }
    }
}

fn literal_to_json(lit: &Literal, as_text: bool) -> SqlResult<serde_json::Value> {
    match (lit, as_text) {
        (Literal::Null, _) => Ok(serde_json::Value::Null),
        (Literal::Text(s), true) => Ok(serde_json::Value::String(s.clone())),
        (Literal::Text(s), false) => {
            // `->` text literals are canonical strings in JSON;
            // the user writes `'abc'` and gets a JSON string.
            Ok(serde_json::Value::String(s.clone()))
        }
        (Literal::Number(s), as_text) => {
            if as_text {
                Ok(serde_json::Value::String(s.clone()))
            } else {
                // Parse as f64 if fractional, else i64.
                if s.contains('.') {
                    let n: f64 = s.parse().map_err(|_| SqlError::Type(
                        format!("invalid numeric for JSON mutation: {s}")))?;
                    Ok(serde_json::Number::from_f64(n)
                        .map(serde_json::Value::Number)
                        .unwrap_or(serde_json::Value::Null))
                } else {
                    let n: i64 = s.parse().map_err(|_| SqlError::Type(
                        format!("invalid integer for JSON mutation: {s}")))?;
                    Ok(serde_json::Value::Number(n.into()))
                }
            }
        }
        (Literal::Bool(b), true) => Ok(serde_json::Value::String(
            if *b { "true".into() } else { "false".into() })),
        (Literal::Bool(b), false) => Ok(serde_json::Value::Bool(*b)),
        (Literal::Default, _) => Err(SqlError::Syntax(
            "DEFAULT is not supported in a JSON path assignment".into())),
    }
}

/// ADR 0191 + ADR 0196 — one segment of a JSON path chain.
/// A key addresses an object member; an index addresses an
/// array element from the front; `IndexFromEnd(n)` addresses
/// `arr.len() - n` (so `n = 1` is the last element).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JsonSegment {
    Key(String),
    Index(usize),
    IndexFromEnd(usize),
    /// ADR 0197 + ADR 0202 — Python-style end-exclusive slice.
    /// `None` on `start` means from the beginning; `None` on
    /// `end` means to the array length. `start > end` (when
    /// both present) is rejected at parse. Reading a slice
    /// returns a sub-array (as JSON); non-array values
    /// resolve to `None` → `Value::Null`. Slice mutation is
    /// not supported.
    Slice { start: Option<usize>, end: Option<usize> },
}

impl JsonSegment {
    fn apply<'v>(&self, value: &'v serde_json::Value) -> Option<&'v serde_json::Value> {
        match self {
            JsonSegment::Key(k) => value.get(k.as_str()),
            JsonSegment::Index(i) => value.get(*i),
            JsonSegment::IndexFromEnd(n) => {
                let arr = value.as_array()?;
                let len = arr.len();
                if *n == 0 || *n > len { return None; }
                arr.get(len - *n)
            }
            JsonSegment::Slice { .. } => {
                // Slice reads produce an **owned** sub-array;
                // `apply` returns a reference and so cannot
                // represent it here.  `apply_slice` handles
                // this case in `apply_projection`.
                None
            }
        }
    }

    /// ADR 0197 — slice extraction. Returns `Some(array)` when
    /// `value` is an array; `None` otherwise (resolves to NULL
    /// in `apply_projection`). The returned array is a fresh
    /// clone of the sub-slice.
    fn apply_slice(&self, value: &serde_json::Value) -> Option<serde_json::Value> {
        let JsonSegment::Slice { start, end } = self else { return None; };
        let arr = value.as_array()?;
        let len = arr.len();
        // ADR 0202 — None bounds mean "from the start" / "to the end".
        let lo = start.unwrap_or(0).min(len);
        let hi = end.unwrap_or(len).min(len);
        if lo >= hi {
            return Some(serde_json::Value::Array(Vec::new()));
        }
        Some(serde_json::Value::Array(arr[lo..hi].to_vec()))
    }

    /// ADR 0196 — resolve a negative index against a concrete
    /// array for mutation. Returns `None` for out-of-bounds
    /// and for `Slice` / `Key` segments (not mutatable).
    pub(crate) fn resolve_for_array(&self, len: usize) -> Option<usize> {
        match self {
            JsonSegment::Index(i) if *i < len => Some(*i),
            JsonSegment::IndexFromEnd(n) if *n > 0 && *n <= len => Some(len - *n),
            _ => None,
        }
    }

    fn render(&self, out: &mut String) {
        match self {
            JsonSegment::Key(k) => {
                out.push('\'');
                out.push_str(k);
                out.push('\'');
            }
            JsonSegment::Index(i) => {
                use std::fmt::Write;
                let _ = write!(out, "{i}");
            }
            JsonSegment::IndexFromEnd(n) => {
                use std::fmt::Write;
                let _ = write!(out, "-{n}");
            }
            JsonSegment::Slice { start, end } => {
                use std::fmt::Write;
                if let Some(s) = start { let _ = write!(out, "{s}"); }
                out.push(':');
                if let Some(e) = end { let _ = write!(out, "{e}"); }
            }
        }
    }
}

/// ADR 0186 + ADR 0189 + ADR 0191 — SELECT projection element.
/// Either a bare column or a chained JSON path extraction with
/// one or more segments (object keys or array indices).
/// `as_text` applies to the **final** step only; intermediate
/// steps are always `->`. A `->>` can only appear last.
#[derive(Clone, Debug)]
pub(crate) enum Projection {
    Column(String),
    JsonPath {
        column: String,
        path: Vec<JsonSegment>,
        as_text: bool,
        /// ADR 0194 — rendered as `col #> '{…}'` instead of
        /// the arrow chain. The runtime semantics are
        /// identical; the flag only affects label display and
        /// keeps error messages honest about which syntax the
        /// caller used.
        container_syntax: bool,
    },
}

impl Projection {
    pub(crate) fn source_column(&self) -> &str {
        match self {
            Projection::Column(c) => c,
            Projection::JsonPath { column, .. } => column,
        }
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Projection::Column(c) => c.clone(),
            Projection::JsonPath { column, path, as_text, container_syntax } => {
                let mut out = column.clone();
                if *container_syntax {
                    out.push_str(if *as_text { "#>>" } else { "#>" });
                    out.push_str("'{");
                    for (i, seg) in path.iter().enumerate() {
                        if i > 0 { out.push(','); }
                        use std::fmt::Write;
                        match seg {
                            JsonSegment::Key(k) => out.push_str(k),
                            JsonSegment::Index(n) => { let _ = write!(out, "{n}"); }
                            JsonSegment::IndexFromEnd(n) => { let _ = write!(out, "-{n}"); }
                            JsonSegment::Slice { start, end } => {
                                if let Some(s) = start { let _ = write!(out, "{s}"); }
                                out.push(':');
                                if let Some(e) = end { let _ = write!(out, "{e}"); }
                            }
                        }
                    }
                    out.push_str("}'");
                } else if let Some((last, mids)) = path.split_last() {
                    for seg in mids {
                        out.push_str("->");
                        seg.render(&mut out);
                    }
                    let op = if *as_text { "->>" } else { "->" };
                    out.push_str(op);
                    last.render(&mut out);
                }
                out
            }
        }
    }
}

/// ADR 0213 — recognize `COUNT ( * )` at the head of a SELECT.
/// Returns true after consuming all three tokens; returns false
/// without advancing the parser if the next token is not `COUNT`.
/// A `COUNT` not followed by `( * )` is a parse error (reserved
/// shape, preserves room for `COUNT(col)` / `COUNT(DISTINCT …)`
/// later).
pub(crate) fn consume_count_star(p: &mut Parser) -> SqlResult<bool> {
    if !p.consume_word("count") { return Ok(false); }
    p.symbol('(')?;
    if !p.consume_symbol('*') {
        return Err(SqlError::Unsupported(
            "only COUNT(*) is supported today; COUNT(col) / DISTINCT reserved".into()));
    }
    p.symbol(')')?;
    Ok(true)
}

/// ADR 0186 + ADR 0189 — parse a projection: identifier,
/// optionally followed by one or more `-> 'key'` / `->> 'key'`
/// segments. Intermediate segments must be `->`; a `->>`
/// segment can only appear at the end (SQL-standard: text
/// extraction terminates the path).
pub(crate) fn parse_projection(p: &mut Parser) -> SqlResult<Projection> {
    let column = p.word()?;
    parse_projection_tail(p, column)
}

/// ADR 0190 — parse the arrow chain after an already-consumed
/// column identifier. Used by both `parse_projection` and the
/// WHERE-side JSON path predicate parser so the chain grammar
/// has exactly one implementation.
pub(crate) fn parse_projection_tail(p: &mut Parser, column: String) -> SqlResult<Projection> {
    let mut path: Vec<JsonSegment> = Vec::new();
    let mut as_text = false;
    // ADR 0194 — first-segment branch may be a #> / #>>
    // container-path operator. These do not chain: one
    // #> / #>> must own the whole path for that column.
    match p.peek() {
        Some(Token::Hash) | Some(Token::HashText) => {
            let final_text = matches!(p.peek(), Some(Token::HashText));
            p.advance();
            path = parse_container_path(p)?;
            if path.is_empty() {
                return Err(SqlError::Syntax(
                    "container-path operator requires a non-empty '{…}' body".into()));
            }
            return Ok(Projection::JsonPath {
                column, path, as_text: final_text, container_syntax: true,
            });
        }
        _ => {}
    }
    loop {
        let final_text = match p.peek() {
            Some(Token::ArrowText) => { p.advance(); true }
            Some(Token::Arrow) => { p.advance(); false }
            _ => break,
        };
        if as_text {
            return Err(SqlError::Unsupported(
                "`->>` JSON path terminator cannot be followed by more segments".into()));
        }
        // ADR 0191 / ADR 0196 — a segment may be a quoted
        // string key, a non-negative integer array index, or a
        // negative integer from-end index (`-1` = last).  The
        // expression-mode lexer splits `-N` into `Symbol('-')`
        // and `Number(N)`; both forms land here as the same
        // `IndexFromEnd` segment.
        let segment = match p.peek().cloned() {
            Some(Token::Text(s)) => { p.advance(); JsonSegment::Key(s) }
            // ADR 0202 — leading `:` opens a slice with an
            // omitted start; `:` alone means the whole array.
            Some(Token::Symbol(':')) => {
                p.advance();
                let end = parse_optional_slice_bound(p)?;
                JsonSegment::Slice { start: None, end }
            }
            Some(Token::Number(s)) if !s.contains('.') && !s.starts_with('-') => {
                p.advance();
                // ADR 0197 + ADR 0202 — slice form `start:end`,
                // open-ended on either side.
                if matches!(p.peek(), Some(Token::Symbol(':'))) {
                    p.advance();
                    let start: usize = s.parse()
                        .map_err(|_| SqlError::Syntax(
                            format!("JSON slice start {s} is not a valid usize")))?;
                    let end = parse_optional_slice_bound(p)?;
                    if let Some(e) = end {
                        if start > e {
                            return Err(SqlError::Syntax(format!(
                                "JSON slice start {start} must be <= end {e}")));
                        }
                    }
                    JsonSegment::Slice { start: Some(start), end }
                } else {
                    let idx: usize = s.parse()
                        .map_err(|_| SqlError::Syntax(
                            format!("JSON path array index {s} is not a valid usize")))?;
                    JsonSegment::Index(idx)
                }
            }
            Some(Token::Number(s)) if !s.contains('.') && s.starts_with('-') => {
                p.advance();
                let abs = s.trim_start_matches('-');
                let n: usize = abs.parse()
                    .map_err(|_| SqlError::Syntax(
                        format!("JSON path negative index {s} is not a valid isize")))?;
                if n == 0 {
                    return Err(SqlError::Syntax(
                        "JSON path negative index must be nonzero (-1 is the last element)".into()));
                }
                JsonSegment::IndexFromEnd(n)
            }
            Some(Token::Symbol('-')) => {
                p.advance();
                let num = match p.peek().cloned() {
                    Some(Token::Number(s)) if !s.contains('.') && !s.starts_with('-') => {
                        p.advance();
                        s
                    }
                    _ => return Err(SqlError::Syntax(
                        "JSON path `-` must be followed by a non-negative integer".into())),
                };
                let n: usize = num.parse()
                    .map_err(|_| SqlError::Syntax(
                        format!("JSON path negative index -{num} is not a valid isize")))?;
                if n == 0 {
                    return Err(SqlError::Syntax(
                        "JSON path negative index must be nonzero (-1 is the last element)".into()));
                }
                JsonSegment::IndexFromEnd(n)
            }
            _ => return Err(SqlError::Syntax(
                "JSON path segment must be a single-quoted string key or an integer index".into())),
        };
        path.push(segment);
        as_text = final_text;
    }
    if path.is_empty() {
        return Ok(Projection::Column(column));
    }
    Ok(Projection::JsonPath { column, path, as_text, container_syntax: false })
}

/// ADR 0202 — parse the optional end of a slice. The token
/// after the `:` is either a non-negative integer or the
/// start of the next segment / an arrow / a close paren /
/// a comma. If it is not a number we leave the token in
/// place and return `None`.
fn parse_optional_slice_bound(p: &mut Parser) -> SqlResult<Option<usize>> {
    match p.peek().cloned() {
        Some(Token::Number(e)) if !e.contains('.') && !e.starts_with('-') => {
            p.advance();
            let v: usize = e.parse()
                .map_err(|_| SqlError::Syntax(
                    format!("JSON slice bound {e} is not a valid usize")))?;
            Ok(Some(v))
        }
        _ => Ok(None),
    }
}

/// ADR 0194 — parse the RHS of `#>` / `#>>` as a single
/// TEXT literal with the shape `'{seg,seg,...}'` where each
/// segment is a bareword (object key) or a non-negative
/// integer (array index). Whitespace around commas and
/// braces is ignored. An empty `{}` is rejected.
fn parse_container_path(p: &mut Parser) -> SqlResult<Vec<JsonSegment>> {
    let text = match p.peek().cloned() {
        Some(Token::Text(s)) => { p.advance(); s }
        _ => return Err(SqlError::Syntax(
            "container-path operator requires a single-quoted '{…}' text literal".into())),
    };
    let trimmed = text.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| SqlError::Syntax(
            format!("container-path literal must be wrapped in '{{…}}'; got {text:?}")))?;
    let mut out = Vec::new();
    if inner.trim().is_empty() {
        return Ok(out);
    }
    for raw in inner.split(',') {
        let seg = raw.trim();
        if seg.is_empty() {
            return Err(SqlError::Syntax(
                "container-path segments must be non-empty".into()));
        }
        // ADR 0197 + ADR 0202 — slice segment `start:end`,
        // open-ended on either side.
        if let Some((start_s, end_s)) = seg.split_once(':') {
            let start_s = start_s.trim();
            let end_s = end_s.trim();
            let parse_bound = |s: &str, kind: &str| -> SqlResult<Option<usize>> {
                if s.is_empty() { Ok(None) }
                else if s.bytes().all(|b| b.is_ascii_digit()) {
                    s.parse::<usize>().map(Some).map_err(|_| SqlError::Syntax(
                        format!("container-path slice {kind} {s:?} is not a valid usize")))
                } else {
                    Err(SqlError::Syntax(format!(
                        "container-path slice {kind} must be a non-negative integer or empty; got {s:?}")))
                }
            };
            let start = parse_bound(start_s, "start")?;
            let end = parse_bound(end_s, "end")?;
            if let (Some(s), Some(e)) = (start, end) {
                if s > e {
                    return Err(SqlError::Syntax(format!(
                        "container-path slice start {s} must be <= end {e}")));
                }
            }
            out.push(JsonSegment::Slice { start, end });
            continue;
        }
        // ADR 0196 — negative integer segment resolves to
        // IndexFromEnd. `-0` is invalid.
        if let Some(abs) = seg.strip_prefix('-') {
            if abs.bytes().all(|b| b.is_ascii_digit()) && !abs.is_empty() {
                let n: usize = abs.parse().map_err(|_| SqlError::Syntax(
                    format!("container-path negative index {seg:?} is not a valid isize")))?;
                if n == 0 {
                    return Err(SqlError::Syntax(
                        "container-path negative index must be nonzero".into()));
                }
                out.push(JsonSegment::IndexFromEnd(n));
                continue;
            }
        }
        if seg.bytes().all(|b| b.is_ascii_digit()) {
            let idx: usize = seg.parse().map_err(|_| SqlError::Syntax(
                format!("container-path index {seg:?} is not a valid usize")))?;
            out.push(JsonSegment::Index(idx));
        } else {
            out.push(JsonSegment::Key(seg.to_string()));
        }
    }
    Ok(out)
}

/// ADR 0190 / ADR 0195 — SELECT WHERE clause shape. PK
/// equality uses the fast-lookup path; a JSON-path predicate
/// (equality or any ordering comparison) runs a full scan
/// with a per-row filter.
#[derive(Clone, Debug)]
pub(crate) enum JsonMatcher {
    /// ADR 0195 — scalar comparison against a single literal.
    Compare { op: crate::model::CheckOp, literal: Value },
    /// ADR 0206 — membership: row matches when the extracted
    /// value equals any of the listed literals.
    In { literals: Vec<Value> },
    /// ADR 0206 — negated membership.
    NotIn { literals: Vec<Value> },
    /// ADR 0207 — explicit NULL test. True when the extracted
    /// value is `Value::Null`.
    IsNull,
    /// ADR 0207 — explicit NOT NULL test.
    IsNotNull,
    /// ADR 0208 — inclusive range: low <= extracted <= high.
    Between { low: Value, high: Value },
    /// ADR 0208 — negated range.
    NotBetween { low: Value, high: Value },
    /// ADR 0209 + ADR 0211 — SQL-style pattern match: `%` = any
    /// run, `_` = one character; an optional ESCAPE character
    /// quotes the next literal `%` or `_`. Case-sensitive. Text
    /// pattern stored as-is; matching converts the extracted
    /// value to a string.
    Like { pattern: String, escape: Option<char> },
    /// ADR 0209 + ADR 0211 — negated pattern match.
    NotLike { pattern: String, escape: Option<char> },
    /// ADR 0210 + ADR 0211 — case-insensitive pattern match.
    /// Pattern and extracted text are both ASCII-lowercased
    /// before the LIKE walk; the escape character (if any) is
    /// lowercased alongside the pattern.
    Ilike { pattern_lower: String, escape: Option<char> },
    /// ADR 0210 + ADR 0211 — negated case-insensitive pattern.
    NotIlike { pattern_lower: String, escape: Option<char> },
}

#[derive(Clone, Debug)]
pub(crate) struct JsonPredicate {
    pub(crate) projection: Projection,
    pub(crate) matcher: JsonMatcher,
}

#[derive(Clone, Debug)]
pub(crate) enum SelectWhere {
    KeyPredicate(Vec<Value>),
    /// ADR 0190 + ADR 0204 + ADR 0205 — JSON-path WHERE in
    /// disjunctive normal form: the outer `Vec` is OR-joined;
    /// each inner `Vec` is an AND-group. A row matches when
    /// at least one AND-group holds. Single-predicate and
    /// AND-only queries remain a single outer element.
    JsonPathPredicates(Vec<Vec<JsonPredicate>>),
}

/// ADR 0190 — parse the full WHERE clause for a SELECT. The
/// branch is chosen at the first column: a bare column
/// followed by `=` dispatches to the existing PK-equality
/// path; a column followed by `->` / `->>` dispatches to the
/// JSON-path predicate path. Any other token is a parse error.
///
/// For PK predicates this preserves the existing
/// `key_predicate` grammar (full PK equality binding). For
/// JSON paths it accepts one `col path-chain = literal` pair
/// — no AND, no mixing with PK predicates in this scope.
pub(crate) fn parse_select_where(p: &mut Parser, table: &Table) -> SqlResult<SelectWhere> {
    p.expect("where")?;
    let saved = p.save_pos();
    let column_name = p.word()?;
    let is_json_path = matches!(p.peek(),
        Some(Token::Arrow) | Some(Token::ArrowText)
        | Some(Token::Hash) | Some(Token::HashText));
    if is_json_path {
        // ADR 0205 — DNF: groups are AND-groups, joined by OR.
        let mut groups: Vec<Vec<JsonPredicate>> = Vec::new();
        let mut current: Vec<JsonPredicate> = Vec::new();
        let mut col = column_name;
        loop {
            let projection = parse_projection_tail(p, col)?;
            locate(table, projection.source_column())?;
            // ADR 0206 — IN (…) / NOT IN (…) before the scalar
            // comparison dispatch. NOT must be immediately
            // followed by IN; a bare NOT is not accepted here.
            let as_text = match &projection {
                Projection::Column(_) => unreachable!("json path has non-empty chain"),
                Projection::JsonPath { as_text, .. } => *as_text,
            };
            let matcher = if p.consume_word("in") {
                JsonMatcher::In { literals: parse_literal_list(p, as_text)? }
            } else if p.consume_word("like") {
                let pattern = parse_like_pattern(p)?;
                let escape = parse_optional_escape(p)?;
                JsonMatcher::Like { pattern, escape }
            } else if p.consume_word("ilike") {
                let pattern = parse_like_pattern(p)?.to_ascii_lowercase();
                let escape = parse_optional_escape(p)?.map(|c| c.to_ascii_lowercase());
                JsonMatcher::Ilike { pattern_lower: pattern, escape }
            } else if p.consume_word("between") {
                parse_between_tail(p, as_text, /*negated=*/false)?
            } else if p.consume_word("is") {
                // ADR 0207 — IS NULL / IS NOT NULL. `NOT` here
                // belongs to the IS test; it does not flip to IN.
                let negated = p.consume_word("not");
                if !p.consume_word("null") {
                    return Err(SqlError::Unsupported(
                        "IS [NOT] NULL is the only IS-form supported for JSON paths".into()));
                }
                if negated { JsonMatcher::IsNotNull } else { JsonMatcher::IsNull }
            } else if p.consume_word("not") {
                // ADR 0206 / 0208 / 0209 — NOT IN / NOT BETWEEN / NOT LIKE.
                if p.consume_word("in") {
                    JsonMatcher::NotIn { literals: parse_literal_list(p, as_text)? }
                } else if p.consume_word("between") {
                    parse_between_tail(p, as_text, /*negated=*/true)?
                } else if p.consume_word("like") {
                    let pattern = parse_like_pattern(p)?;
                    let escape = parse_optional_escape(p)?;
                    JsonMatcher::NotLike { pattern, escape }
                } else if p.consume_word("ilike") {
                    let pattern = parse_like_pattern(p)?.to_ascii_lowercase();
                    let escape = parse_optional_escape(p)?.map(|c| c.to_ascii_lowercase());
                    JsonMatcher::NotIlike { pattern_lower: pattern, escape }
                } else {
                    return Err(SqlError::Unsupported(
                        "NOT must be followed by IN, BETWEEN, LIKE, or ILIKE in a JSON-path predicate".into()));
                }
            } else {
                let op = p.word_or_symbol_pair_for_check()?;
                let literal = p.literal()?;
                let value = coerce_where_literal(&literal, as_text)?;
                JsonMatcher::Compare { op, literal: value }
            };
            current.push(JsonPredicate { projection, matcher });
            // Either continue the current AND-group, start a
            // new OR-group, or stop.
            let next = if p.consume_word("and") { 'a' }
                else if p.consume_word("or") { 'o' }
                else { 'e' };
            if next == 'e' { break; }
            col = p.word()?;
            let next_json = matches!(p.peek(),
                Some(Token::Arrow) | Some(Token::ArrowText)
                | Some(Token::Hash) | Some(Token::HashText));
            if !next_json {
                return Err(SqlError::Unsupported(
                    "AND/OR after a JSON-path predicate must be another JSON-path predicate".into()));
            }
            if next == 'o' {
                groups.push(std::mem::take(&mut current));
            }
        }
        groups.push(current);
        Ok(SelectWhere::JsonPathPredicates(groups))
    } else {
        // Rewind and dispatch to the existing PK grammar.
        p.restore_pos(saved);
        // key_predicate expects the cursor just AFTER WHERE.
        // We've already consumed WHERE above; reconstruct the
        // old signature by rewinding one token for the WHERE
        // keyword as well.
        // Simpler: re-implement the PK walk here using the
        // same loop as `Parser::predicate`.
        let mut tests = Vec::<(String, Literal)>::new();
        loop {
            let column = p.word()?;
            p.symbol('=')?;
            tests.push((column, p.literal()?));
            if !p.consume_word("and") { break; }
        }
        if tests.len() != table.primary_key.columns.len() {
            return Err(SqlError::Unsupported("WHERE must match every primary-key component".into()));
        }
        let mut key = BTreeMap::<String, Value>::new();
        for (name, literal) in tests {
            if !table.primary_key.columns.contains(&name) || key.contains_key(&name) {
                return Err(SqlError::Unsupported("WHERE must be equality on distinct primary-key columns".into()));
            }
            let column = &table.columns[locate(table, &name)?];
            key.insert(name, coerce(&literal, column)?);
        }
        let ordered: Vec<Value> = table.primary_key.columns.iter()
            .map(|name| key.remove(name).expect("validated"))
            .collect();
        Ok(SelectWhere::KeyPredicate(ordered))
    }
}

/// ADR 0209 — parse a LIKE / NOT LIKE pattern. Must be a quoted
/// text literal. Numeric or other literal forms are rejected
/// (SQL-standard LIKE takes a character-string pattern).
fn parse_like_pattern(p: &mut Parser) -> SqlResult<String> {
    match p.literal()? {
        Literal::Text(s) => Ok(s),
        _ => Err(SqlError::Syntax(
            "LIKE / NOT LIKE requires a single-quoted text pattern".into())),
    }
}

/// ADR 0211 — parse an optional `ESCAPE 'c'` suffix after a
/// LIKE / ILIKE pattern. The escape must be a single-character
/// text literal; empty or multi-character strings are rejected.
fn parse_optional_escape(p: &mut Parser) -> SqlResult<Option<char>> {
    if !p.consume_word("escape") { return Ok(None); }
    let lit = p.literal()?;
    let s = match lit {
        Literal::Text(s) => s,
        _ => return Err(SqlError::Syntax(
            "ESCAPE requires a single-character text literal".into())),
    };
    let mut chars = s.chars();
    let c = chars.next().ok_or_else(|| SqlError::Syntax(
        "ESCAPE requires exactly one character".into()))?;
    if chars.next().is_some() {
        return Err(SqlError::Syntax(
            "ESCAPE requires exactly one character".into()));
    }
    Ok(Some(c))
}

/// ADR 0209 + ADR 0211 — SQL LIKE semantics: `%` matches zero or
/// more characters, `_` matches exactly one character. If an
/// `escape` character is set, it quotes the next literal `%`,
/// `_`, or escape character itself. Case-sensitive.
pub(crate) fn like_matches(pattern: &str, value: &str, escape: Option<char>) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = value.chars().collect();
    fn go(pat: &[char], text: &[char], escape: Option<char>) -> bool {
        if pat.is_empty() { return text.is_empty(); }
        // ADR 0211 — escape char forces the next pattern char
        // to match literally.
        if let Some(e) = escape {
            if pat[0] == e {
                if pat.len() < 2 { return false; }
                if text.is_empty() || text[0] != pat[1] { return false; }
                return go(&pat[2..], &text[1..], escape);
            }
        }
        match pat[0] {
            '%' => {
                let mut i = 1;
                while i < pat.len() && pat[i] == '%' { i += 1; }
                if i == pat.len() { return true; }
                for start in 0..=text.len() {
                    if go(&pat[i..], &text[start..], escape) { return true; }
                }
                false
            }
            '_' => {
                if text.is_empty() { return false; }
                go(&pat[1..], &text[1..], escape)
            }
            c => {
                if text.is_empty() || text[0] != c { return false; }
                go(&pat[1..], &text[1..], escape)
            }
        }
    }
    go(&pat, &text, escape)
}

/// ADR 0209 — extract a string view of a value for LIKE
/// matching. Handles `Value::String` directly; a `Value::Json`
/// string scalar is unwrapped (JSON strings are quoted in
/// storage). Any other variant yields `None` so the predicate
/// evaluates to false.
fn like_source(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Json(j) => {
            if let Ok(serde_json::Value::String(s)) = serde_json::from_str(j) {
                Some(s)
            } else { None }
        }
        _ => None,
    }
}

/// ADR 0208 — parse `low AND high` as the RHS of a BETWEEN /
/// NOT BETWEEN predicate. The `AND` here is a grammar keyword,
/// not a DNF separator.  Inclusive on both ends. `low > high`
/// is rejected at parse (empty range would be surprising to
/// readers).
fn parse_between_tail(p: &mut Parser, as_text: bool, negated: bool) -> SqlResult<JsonMatcher> {
    let low_lit = p.literal()?;
    let low = coerce_where_literal(&low_lit, as_text)?;
    if !p.consume_word("and") {
        return Err(SqlError::Syntax(
            "BETWEEN requires `AND` between low and high".into()));
    }
    let high_lit = p.literal()?;
    let high = coerce_where_literal(&high_lit, as_text)?;
    if !matches!(low, Value::Null) && !matches!(high, Value::Null)
        && compare_values_for_check(&low, &high) == std::cmp::Ordering::Greater
    {
        return Err(SqlError::Syntax(
            "BETWEEN low must not exceed high".into()));
    }
    Ok(if negated {
        JsonMatcher::NotBetween { low, high }
    } else {
        JsonMatcher::Between { low, high }
    })
}

/// ADR 0206 — parse `( lit, lit, … )` as the RHS of an IN /
/// NOT IN predicate. Coerces each literal with the same
/// `as_text` convention as a scalar JSON-path predicate. At
/// least one value is required.
fn parse_literal_list(p: &mut Parser, as_text: bool) -> SqlResult<Vec<Value>> {
    p.symbol('(')?;
    let mut values = Vec::new();
    loop {
        let literal = p.literal()?;
        values.push(coerce_where_literal(&literal, as_text)?);
        if !p.consume_symbol(',') { break; }
    }
    p.symbol(')')?;
    if values.is_empty() {
        return Err(SqlError::Syntax(
            "IN / NOT IN requires at least one value".into()));
    }
    Ok(values)
}

/// ADR 0190 — coerce the RHS of a JSON path equality. For
/// `->>` the comparison is against TEXT; the literal must be
/// a quoted string. For `->` the comparison is against JSON;
/// the literal is serialized as JSON for a bytewise equality
/// against the stored sub-value.
fn coerce_where_literal(lit: &Literal, as_text: bool) -> SqlResult<Value> {
    match (lit, as_text) {
        (Literal::Null, _) => Ok(Value::Null),
        (Literal::Text(s), true) => Ok(Value::String(s.clone())),
        (Literal::Text(s), false) => {
            // Parse the literal as JSON (strings are quoted).
            // For ergonomics we also accept a bare word literal
            // on the user's side, but at this point the lexer
            // has already resolved it to `Literal::Text`, so we
            // just wrap it as a JSON string value.
            Ok(Value::Json(serde_json::to_string(s)
                .map_err(|e| SqlError::Type(format!("json render: {e}")))?))
        }
        (Literal::Number(s), as_text) => {
            if as_text {
                Ok(Value::String(s.clone()))
            } else {
                // Numeric literal becomes its JSON serialization.
                Ok(Value::Json(s.clone()))
            }
        }
        (Literal::Bool(b), true) => Ok(Value::String(if *b { "true".into() } else { "false".into() })),
        (Literal::Bool(b), false) => Ok(Value::Json(if *b { "true".into() } else { "false".into() })),
        (Literal::Default, _) => Err(SqlError::Syntax(
            "DEFAULT is not a WHERE-side literal".into())),
    }
}

/// ADR 0190 + ADR 0195 — filter a row slice against a
/// JSON-path comparison predicate. Returns rows whose
/// projected value satisfies `op` against `literal`. NULL on
/// either side never matches (SQL three-valued logic — a
/// comparison with NULL is UNKNOWN, treated as not-true).
pub(crate) fn apply_where_filter(
    rows: Vec<Vec<Value>>,
    table: &Table,
    groups: &[Vec<JsonPredicate>],
) -> SqlResult<Vec<Vec<Value>>> {
    // ADR 0205 — DNF evaluation: outer OR, inner AND. Resolve
    // column positions once per group.
    let positions: Vec<Vec<usize>> = groups.iter().map(|group|
        group.iter()
            .map(|pred| locate(table, pred.projection.source_column()))
            .collect::<SqlResult<Vec<_>>>()
    ).collect::<SqlResult<Vec<_>>>()?;
    let eval_group =
        |row: &[Value], group: &[JsonPredicate], positions: &[usize]| -> SqlResult<bool> {
            for (pred, position) in group.iter().zip(positions) {
                let extracted = apply_projection(&pred.projection, row[*position].clone())?;
                // ADR 0207 — IS [NOT] NULL evaluates NULL
                // directly; every other matcher short-circuits
                // on NULL-extracted under three-valued logic.
                match &pred.matcher {
                    JsonMatcher::IsNull => {
                        if !matches!(extracted, Value::Null) { return Ok(false); }
                        continue;
                    }
                    JsonMatcher::IsNotNull => {
                        if matches!(extracted, Value::Null) { return Ok(false); }
                        continue;
                    }
                    _ => {}
                }
                if matches!(extracted, Value::Null) {
                    return Ok(false);
                }
                let satisfied = match &pred.matcher {
                    JsonMatcher::Compare { op, literal } => {
                        if matches!(literal, Value::Null) { return Ok(false); }
                        let ord = compare_values_for_check(&extracted, literal);
                        match op {
                            crate::model::CheckOp::Eq => ord == std::cmp::Ordering::Equal,
                            crate::model::CheckOp::Ne => ord != std::cmp::Ordering::Equal,
                            crate::model::CheckOp::Lt => ord == std::cmp::Ordering::Less,
                            crate::model::CheckOp::Le => ord != std::cmp::Ordering::Greater,
                            crate::model::CheckOp::Gt => ord == std::cmp::Ordering::Greater,
                            crate::model::CheckOp::Ge => ord != std::cmp::Ordering::Less,
                        }
                    }
                    // ADR 0206 — NULL literals in the IN list
                    // never match (SQL three-valued logic).
                    JsonMatcher::In { literals } => literals.iter().any(|lit|
                        !matches!(lit, Value::Null)
                            && compare_values_for_check(&extracted, lit)
                                == std::cmp::Ordering::Equal),
                    JsonMatcher::NotIn { literals } => literals.iter().all(|lit|
                        !matches!(lit, Value::Null)
                            && compare_values_for_check(&extracted, lit)
                                != std::cmp::Ordering::Equal),
                    JsonMatcher::Between { low, high } => {
                        if matches!(low, Value::Null) || matches!(high, Value::Null) {
                            false
                        } else {
                            compare_values_for_check(&extracted, low)
                                != std::cmp::Ordering::Less
                            && compare_values_for_check(&extracted, high)
                                != std::cmp::Ordering::Greater
                        }
                    }
                    JsonMatcher::NotBetween { low, high } => {
                        if matches!(low, Value::Null) || matches!(high, Value::Null) {
                            false
                        } else {
                            compare_values_for_check(&extracted, low)
                                == std::cmp::Ordering::Less
                            || compare_values_for_check(&extracted, high)
                                == std::cmp::Ordering::Greater
                        }
                    }
                    JsonMatcher::Like { pattern, escape } => match like_source(&extracted) {
                        Some(s) => like_matches(pattern, &s, *escape),
                        None => false,
                    },
                    JsonMatcher::NotLike { pattern, escape } => match like_source(&extracted) {
                        Some(s) => !like_matches(pattern, &s, *escape),
                        None => false,
                    },
                    JsonMatcher::Ilike { pattern_lower, escape } => match like_source(&extracted) {
                        Some(s) => like_matches(pattern_lower, &s.to_ascii_lowercase(), *escape),
                        None => false,
                    },
                    JsonMatcher::NotIlike { pattern_lower, escape } => match like_source(&extracted) {
                        Some(s) => !like_matches(pattern_lower, &s.to_ascii_lowercase(), *escape),
                        None => false,
                    },
                    JsonMatcher::IsNull | JsonMatcher::IsNotNull =>
                        unreachable!("handled above the NULL guard"),
                };
                if !satisfied { return Ok(false); }
            }
            Ok(true)
        };
    let mut kept = Vec::with_capacity(rows.len());
    'row: for row in rows {
        for (group, group_positions) in groups.iter().zip(&positions) {
            if eval_group(&row, group, group_positions)? {
                kept.push(row);
                continue 'row;
            }
        }
    }
    Ok(kept)
}

/// ADR 0186 + ADR 0189 — extract a chained JSON path from a
/// cell. Each segment walks one key into the parsed JSON;
/// missing / NULL at any step short-circuits to `Value::Null`.
/// `as_text` controls only the final rendering.
pub(crate) fn apply_projection(pj: &Projection, cell: Value) -> SqlResult<Value> {
    match pj {
        Projection::Column(_) => Ok(cell),
        Projection::JsonPath { column, path, as_text, .. } => {
            let source = match &cell {
                Value::Null => return Ok(Value::Null),
                Value::Json(s) | Value::String(s) => s.as_str(),
                other => return Err(SqlError::Type(format!(
                    "JSON path operator requires a JSON cell; column {column} holds {other:?}"))),
            };
            let mut current: serde_json::Value = serde_json::from_str(source)
                .map_err(|e| SqlError::Type(format!("JSON path source does not parse: {e}")))?;
            for seg in path {
                current = match seg {
                    JsonSegment::Slice { .. } => match seg.apply_slice(&current) {
                        Some(next) => next,
                        None => return Ok(Value::Null),
                    },
                    _ => match seg.apply(&current) {
                        Some(next) => next.clone(),
                        None => return Ok(Value::Null),
                    },
                };
                if matches!(current, serde_json::Value::Null) {
                    return Ok(Value::Null);
                }
            }
            if *as_text {
                Ok(Value::String(match current {
                    serde_json::Value::String(s) => s,
                    other => serde_json::to_string(&other)
                        .map_err(|e| SqlError::Type(format!("JSON path text render: {e}")))?,
                }))
            } else {
                Ok(Value::Json(serde_json::to_string(&current)
                    .map_err(|e| SqlError::Type(format!("JSON path sub-render: {e}")))?))
            }
        }
    }
}

/// ADR 0185 — parse one `CASCADE | SET NULL | RESTRICT |
/// NO ACTION` keyword after `ON DELETE` / `ON UPDATE`.
fn parse_referential_action(p: &mut Parser) -> SqlResult<crate::model::ReferentialAction> {
    use crate::model::ReferentialAction;
    if p.consume_word("cascade") { return Ok(ReferentialAction::Cascade); }
    if p.consume_word("set") {
        p.expect("null")?;
        return Ok(ReferentialAction::SetNull);
    }
    if p.consume_word("restrict") { return Ok(ReferentialAction::Restrict); }
    if p.consume_word("no") {
        p.expect("action")?;
        return Ok(ReferentialAction::Restrict);
    }
    Err(SqlError::Syntax(
        "expected CASCADE / SET NULL / RESTRICT / NO ACTION after ON DELETE/UPDATE".into()))
}

/// ADR 0184 — for a child INSERT or FK-column UPDATE, every
/// foreign key on `table` requires a matching parent row.
/// A NULL in any FK column disables that FK for the row
/// (SQL-standard semantics; matches the UNIQUE NULL rule).
pub(crate) fn foreign_key_check(
    engine: &SqlEngine, table: &Table, row: &[Value],
) -> SqlResult<()> {
    if table.foreign_keys.is_empty() { return Ok(()); }
    for fk in &table.foreign_keys {
        let child_positions: Vec<usize> = fk.columns.iter()
            .map(|c| locate(table, c)).collect::<SqlResult<Vec<_>>>()?;
        let projection: Vec<Value> = child_positions.iter().map(|p| row[*p].clone()).collect();
        if projection.iter().any(|v| matches!(v, Value::Null)) {
            continue;
        }
        let snap = engine.catalog.snapshot();
        let parent = snap.tables().find(|t| t.id == fk.referenced_table)
            .ok_or_else(|| SqlError::Resolution(format!("foreign key {} target missing", fk.name)))?;
        let parent_positions: Vec<usize> = fk.referenced_columns.iter()
            .map(|c| parent.columns.iter().position(|col| col.name == *c)
                .ok_or_else(|| SqlError::Resolution(format!("foreign key {} referenced column {c} missing", fk.name))))
            .collect::<SqlResult<Vec<_>>>()?;
        let parent_table = parent.clone();
        drop(snap);
        let parent_store = engine.table_store(&parent_table)?;
        let parent_rows = parent_store.scan()?;
        let hit = parent_rows.iter().any(|r|
            parent_positions.iter().zip(projection.iter())
                .all(|(pos, val)| &r[*pos] == val));
        if !hit {
            return Err(SqlError::ForeignKey {
                constraint: fk.name.clone(), reason: "missing-parent",
            });
        }
    }
    Ok(())
}

/// ADR 0187 — trait abstracting the row-access surface the FK
/// helpers need, so the three session engines
/// (DomainSqlSession / HistorySqlSession / MvccSqlSession) can
/// share the same enforcement code with `SqlEngine`. The
/// implementations wrap each engine's transaction type.
pub(crate) trait TxFkRowAccess {
    fn fk_scan(&mut self, table_id: crate::TableId) -> SqlResult<Vec<Vec<Value>>>;
    fn fk_delete(&mut self, table_id: crate::TableId, pk: &[Value]) -> SqlResult<()>;
    fn fk_replace(&mut self, table_id: crate::TableId, row: &[Value]) -> SqlResult<()>;
}

impl TxFkRowAccess for crate::transaction_domain::MultiTableTransaction<'_> {
    fn fk_scan(&mut self, table_id: crate::TableId) -> SqlResult<Vec<Vec<Value>>> {
        self.scan(table_id).map_err(SqlError::Domain)
    }
    fn fk_delete(&mut self, table_id: crate::TableId, pk: &[Value]) -> SqlResult<()> {
        self.delete(table_id, pk).map_err(SqlError::Domain)
    }
    fn fk_replace(&mut self, table_id: crate::TableId, row: &[Value]) -> SqlResult<()> {
        self.replace(table_id, row).map_err(SqlError::Domain)
    }
}

impl TxFkRowAccess for crate::durable_typed::DurableTypedTransaction<'_> {
    fn fk_scan(&mut self, table_id: crate::TableId) -> SqlResult<Vec<Vec<Value>>> {
        self.scan(table_id).map_err(SqlError::Domain)
    }
    fn fk_delete(&mut self, table_id: crate::TableId, pk: &[Value]) -> SqlResult<()> {
        self.delete(table_id, pk).map_err(SqlError::Domain)
    }
    fn fk_replace(&mut self, table_id: crate::TableId, row: &[Value]) -> SqlResult<()> {
        self.replace(table_id, row).map_err(SqlError::Domain)
    }
}

impl TxFkRowAccess for crate::typed_mvcc::TypedMvccTransaction<'_> {
    fn fk_scan(&mut self, table_id: crate::TableId) -> SqlResult<Vec<Vec<Value>>> {
        self.scan(table_id).map_err(SqlError::Domain)
    }
    fn fk_delete(&mut self, table_id: crate::TableId, pk: &[Value]) -> SqlResult<()> {
        self.delete(table_id, pk).map_err(SqlError::Domain)
    }
    fn fk_replace(&mut self, table_id: crate::TableId, row: &[Value]) -> SqlResult<()> {
        self.replace(table_id, row).map_err(SqlError::Domain)
    }
}

/// ADR 0187 — generic FK parent-exists check. Reads the parent
/// table through the caller-supplied tx so pending inserts in
/// the same session are visible.
pub(crate) fn fk_check_child_row_tx(
    catalog: &crate::persistent_catalog::PersistentCatalog,
    table: &Table,
    row: &[Value],
    tx: &mut dyn TxFkRowAccess,
) -> SqlResult<()> {
    if table.foreign_keys.is_empty() { return Ok(()); }
    let snap = catalog.snapshot();
    for fk in &table.foreign_keys {
        let child_positions: Vec<usize> = fk.columns.iter()
            .map(|c| locate(table, c)).collect::<SqlResult<Vec<_>>>()?;
        let projection: Vec<Value> = child_positions.iter().map(|p| row[*p].clone()).collect();
        if projection.iter().any(|v| matches!(v, Value::Null)) { continue; }
        let parent = snap.tables().find(|t| t.id == fk.referenced_table)
            .ok_or_else(|| SqlError::Resolution(format!("foreign key {} target missing", fk.name)))?;
        let parent_positions: Vec<usize> = fk.referenced_columns.iter()
            .map(|c| parent.columns.iter().position(|col| col.name == *c)
                .ok_or_else(|| SqlError::Resolution(
                    format!("foreign key {} referenced column {c} missing", fk.name))))
            .collect::<SqlResult<Vec<_>>>()?;
        let parent_rows = tx.fk_scan(parent.id)?;
        let hit = parent_rows.iter().any(|r|
            parent_positions.iter().zip(projection.iter())
                .all(|(pos, val)| &r[*pos] == val));
        if !hit {
            return Err(SqlError::ForeignKey {
                constraint: fk.name.clone(), reason: "missing-parent",
            });
        }
    }
    Ok(())
}

/// ADR 0187 — generic parent-DELETE action dispatcher. Mirrors
/// `foreign_key_restrict` but reads / writes through the
/// supplied tx so the delete / replace are visible to the
/// surrounding session.
pub(crate) fn fk_apply_parent_delete_tx(
    catalog: &crate::persistent_catalog::PersistentCatalog,
    parent: &Table,
    parent_pk: &[Value],
    tx: &mut dyn TxFkRowAccess,
) -> SqlResult<()> {
    let snap = catalog.snapshot();
    let parent_pk_positions: Vec<usize> = parent.primary_key.columns.iter()
        .map(|c| locate(parent, c)).collect::<SqlResult<Vec<_>>>()?;
    let mut children: Vec<(Table, crate::model::ForeignKey)> = Vec::new();
    for child in snap.tables() {
        for fk in &child.foreign_keys {
            if fk.referenced_table == parent.id
                && fk.referenced_columns == parent.primary_key.columns
            {
                children.push((child.clone(), fk.clone()));
            }
        }
    }
    drop(snap);
    for (child, fk) in children {
        let child_positions: Vec<usize> = fk.columns.iter()
            .map(|c| locate(&child, c)).collect::<SqlResult<Vec<_>>>()?;
        let child_pk_positions: Vec<usize> = child.primary_key.columns.iter()
            .map(|c| locate(&child, c)).collect::<SqlResult<Vec<_>>>()?;
        let rows = tx.fk_scan(child.id)?;
        for r in rows {
            let matches_pk = child_positions.iter().zip(parent_pk_positions.iter())
                .all(|(ch, pa)| r[*ch] == parent_pk[*pa]);
            if !matches_pk { continue; }
            match fk.on_delete {
                crate::model::ReferentialAction::Restrict => {
                    return Err(SqlError::ForeignKey {
                        constraint: fk.name.clone(), reason: "restrict",
                    });
                }
                crate::model::ReferentialAction::Cascade => {
                    let child_pk: Vec<Value> = child_pk_positions.iter()
                        .map(|p| r[*p].clone()).collect();
                    fk_apply_parent_delete_tx(catalog, &child, &child_pk, tx)?;
                    tx.fk_delete(child.id, &child_pk)?;
                }
                crate::model::ReferentialAction::SetNull => {
                    let mut mutated = r.clone();
                    for pos in &child_positions { mutated[*pos] = Value::Null; }
                    check_row(&child, &mutated)?;
                    fk_check_child_row_tx(catalog, &child, &mutated, tx)?;
                    if !child.unique_keys.is_empty() {
                        let child_pk: Vec<Value> = child_pk_positions.iter()
                            .map(|p| r[*p].clone()).collect();
                        let others = tx.fk_scan(child.id)?.into_iter()
                            .filter(|row| child_pk_positions.iter().zip(child_pk.iter())
                                .any(|(pp, pk_val)| &row[*pp] != pk_val));
                        crate::unique_check::UniqueConstraintChecker::new(&child)
                            .check(&mutated, others)?;
                    }
                    tx.fk_replace(child.id, &mutated)?;
                }
            }
        }
    }
    Ok(())
}

/// ADR 0184 / ADR 0185 — before deleting a parent row, apply
/// each referencing FK's `on_delete` action:
///
/// - `Restrict` (default) — reject the DELETE if any child
///   row references the departing PK.
/// - `Cascade` — delete every matching child row.
/// - `SetNull` — set the FK columns of every matching child
///   row to NULL. The catalog refuses SET NULL on a NOT NULL
///   child column at CREATE, so this can always succeed
///   schema-wise. Each updated child row still re-runs the
///   CHECK / FK / UNIQUE pipeline so downstream constraints
///   can block the cascade.
pub(crate) fn foreign_key_restrict(
    engine: &SqlEngine, parent: &Table, parent_pk: &[Value],
) -> SqlResult<()> {
    let snap = engine.catalog.snapshot();
    let parent_pk_positions: Vec<usize> = parent.primary_key.columns.iter()
        .map(|c| locate(parent, c)).collect::<SqlResult<Vec<_>>>()?;
    let mut children = Vec::new();
    for child in snap.tables() {
        for fk in &child.foreign_keys {
            if fk.referenced_table == parent.id
                && fk.referenced_columns == parent.primary_key.columns
            {
                children.push((child.clone(), fk.clone()));
            }
        }
    }
    drop(snap);
    for (child, fk) in children {
        let child_positions: Vec<usize> = fk.columns.iter()
            .map(|c| locate(&child, c)).collect::<SqlResult<Vec<_>>>()?;
        let child_store = engine.table_store(&child)?;
        let rows = child_store.scan()?;
        let child_pk_positions: Vec<usize> = child.primary_key.columns.iter()
            .map(|c| locate(&child, c)).collect::<SqlResult<Vec<_>>>()?;
        for r in rows {
            let matches_pk = child_positions.iter().zip(parent_pk_positions.iter())
                .all(|(ch, pa)| r[*ch] == parent_pk[*pa]);
            if !matches_pk { continue; }
            match fk.on_delete {
                crate::model::ReferentialAction::Restrict => {
                    return Err(SqlError::ForeignKey {
                        constraint: fk.name.clone(), reason: "restrict",
                    });
                }
                crate::model::ReferentialAction::Cascade => {
                    // Delete the matching child by its PK. This
                    // recursively re-enters foreign_key_restrict
                    // for any grandchildren — SQL-standard
                    // CASCADE semantics.
                    let child_pk: Vec<Value> = child_pk_positions.iter()
                        .map(|p| r[*p].clone()).collect();
                    foreign_key_restrict(engine, &child, &child_pk)?;
                    child_store.delete(&child_pk)?;
                }
                crate::model::ReferentialAction::SetNull => {
                    // Build the mutated row with FK cols nulled.
                    let mut mutated = r.clone();
                    for pos in &child_positions {
                        mutated[*pos] = Value::Null;
                    }
                    // Re-run downstream constraints on the
                    // mutated row (CHECK / FK / UNIQUE).
                    check_row(&child, &mutated)?;
                    foreign_key_check(engine, &child, &mutated)?;
                    if !child.unique_keys.is_empty() {
                        let child_pk: Vec<Value> = child_pk_positions.iter()
                            .map(|p| r[*p].clone()).collect();
                        let others = child_store.scan()?.into_iter()
                            .filter(|row| child_pk_positions.iter().zip(child_pk.iter())
                                .any(|(ppos, pk_val)| &row[*ppos] != pk_val));
                        crate::unique_check::UniqueConstraintChecker::new(&child)
                            .check(&mutated, others)?;
                    }
                    child_store.replace(&mutated)?;
                }
            }
        }
    }
    Ok(())
}

/// ADR 0182 — evaluate every column CHECK on a candidate row.
/// Returns `SqlError::Check` on the first violation. NULL
/// columns skip their checks (SQL-standard three-valued logic:
/// NULL compared to anything is UNKNOWN, treated as not-false).
pub(crate) fn check_row(table: &Table, row: &[Value]) -> SqlResult<()> {
    for (column_name, predicates) in &table.column_checks {
        let position = locate(table, column_name)?;
        let value = &row[position];
        if matches!(value, Value::Null) { continue; }
        for (op, literal) in predicates {
            let ord = compare_values_for_check(value, literal);
            let satisfied = match op {
                crate::model::CheckOp::Eq => ord == std::cmp::Ordering::Equal,
                crate::model::CheckOp::Ne => ord != std::cmp::Ordering::Equal,
                crate::model::CheckOp::Lt => ord == std::cmp::Ordering::Less,
                crate::model::CheckOp::Le => ord != std::cmp::Ordering::Greater,
                crate::model::CheckOp::Gt => ord == std::cmp::Ordering::Greater,
                crate::model::CheckOp::Ge => ord != std::cmp::Ordering::Less,
            };
            if !satisfied {
                return Err(SqlError::Check {
                    column: column_name.clone(),
                    op: op.sql(),
                    literal: format!("{literal:?}"),
                });
            }
        }
    }
    Ok(())
}

fn compare_values_for_check(a: &Value, b: &Value) -> std::cmp::Ordering {
    match (a, b) {
        (Value::Boolean(a), Value::Boolean(b)) => a.cmp(b),
        (Value::Int32(a), Value::Int32(b)) => a.cmp(b),
        (Value::Int64(a), Value::Int64(b)) => a.cmp(b),
        (Value::Decimal(a), Value::Decimal(b)) => a.cmp(b),
        (Value::String(a) | Value::Json(a), Value::String(b) | Value::Json(b)) => a.cmp(b),
        (Value::TimestampTz(a), Value::TimestampTz(b)) => a.cmp(b),
        (Value::Uuid(a), Value::Uuid(b)) => a.cmp(b),
        // Mismatched types cannot occur: the catalog checks
        // literal/column type compatibility at CREATE TABLE.
        _ => std::cmp::Ordering::Equal,
    }
}

pub(crate) fn coerce(lit: &Literal, column: &Column) -> SqlResult<Value> {
    if matches!(lit, Literal::Null) {
        return if column.nullable { Ok(Value::Null) }
        else { Err(SqlError::Type(format!("{} cannot be NULL", column.name))) };
    }
    let invalid = || SqlError::Type(format!("literal does not match column {}", column.name));
    match (&column.data_type, lit) {
        (DataType::Boolean, Literal::Bool(v)) => Ok(Value::Boolean(*v)),
        (DataType::Int32, Literal::Number(s)) if !s.contains('.') =>
            Ok(Value::Int32(s.parse().map_err(|_| invalid())?)),
        (DataType::Int64, Literal::Number(s)) if !s.contains('.') =>
            Ok(Value::Int64(s.parse().map_err(|_| invalid())?)),
        (DataType::Text, Literal::Text(s)) => Ok(Value::String(s.clone())),
        (DataType::Varchar { max_length }, Literal::Text(s))
            if s.len() <= *max_length as usize => Ok(Value::String(s.clone())),
        // ADR 0113 — JSON column accepts a TEXT literal that
        // must parse as valid JSON. ADR 0173 — the coerced
        // value is `Value::Json`, so round-trips preserve the
        // declared type. Invalid JSON is rejected with Type
        // error; no application-level JSON path operators yet.
        (DataType::Json, Literal::Text(s)) => {
            serde_json::from_str::<serde_json::Value>(s)
                .map_err(|e| SqlError::Type(format!("invalid JSON literal: {e}")))?;
            Ok(Value::Json(s.clone()))
        }
        // ADR 0113 — TimestampTz accepts an integer literal
        // (micros since epoch).
        (DataType::TimestampTz, Literal::Number(s)) if !s.contains('.') =>
            Ok(Value::TimestampTz(s.parse().map_err(|_| invalid())?)),
        // ADR 0180 — TimestampTz also accepts an ISO 8601
        // TEXT literal: `YYYY-MM-DDTHH:MM:SS[.ffffff][Z|±HH:MM]`.
        // Exact microsecond precision; unknown format is
        // rejected cleanly as a Type error.
        (DataType::TimestampTz, Literal::Text(s)) => {
            parse_iso8601_micros(s)
                .map(Value::TimestampTz)
                .ok_or_else(|| SqlError::Type(format!(
                    "invalid ISO 8601 timestamp literal: {s:?}")))
        }
        // ADR 0113 — Uuid accepts a 32-hex-char TEXT literal.
        (DataType::Uuid, Literal::Text(s)) => {
            let hex = s.replace('-', "");
            if hex.len() != 32 { return Err(invalid()); }
            let mut bytes = [0u8; 16];
            for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
                let byte_str = std::str::from_utf8(chunk).map_err(|_| invalid())?;
                bytes[i] = u8::from_str_radix(byte_str, 16).map_err(|_| invalid())?;
            }
            Ok(Value::Uuid(bytes))
        }
        (DataType::Decimal { precision, scale }, Literal::Number(s)) => {
            let negative = s.starts_with('-');
            let unsigned = s.trim_start_matches('-');
            let segments = unsigned.split('.').collect::<Vec<_>>();
            if segments.len() > 2 { return Err(invalid()); }
            let fractional = segments.get(1).copied().unwrap_or("");
            if fractional.len() > *scale as usize { return Err(invalid()); }
            let digits = format!("{}{}{}", segments[0], fractional, "0".repeat(*scale as usize - fractional.len()));
            let unsigned = digits.parse::<i128>().map_err(|_| invalid())?;
            let unscaled = if negative { -unsigned } else { unsigned };
            if unscaled.unsigned_abs() >= 10_u128.pow(*precision as u32) {
                return Err(invalid());
            }
            Ok(Value::Decimal(unscaled))
        }
        _ => Err(invalid()),
    }
}

pub(crate) fn locate(table: &Table, field: &str) -> SqlResult<usize> {
    table.columns.iter().position(|c| c.name == field)
        .ok_or_else(|| SqlError::Resolution(format!("unknown column {field}")))
}

pub(crate) fn key_predicate(
    parser: &mut Parser, table: &Table,
) -> SqlResult<Vec<Value>> {
    let entries = parser.predicate()?;
    if entries.len() != table.primary_key.columns.len() {
        return Err(SqlError::Unsupported("WHERE must match every primary-key component".into()));
    }
    let mut key = BTreeMap::<String, Value>::new();
    for (name, literal) in entries {
        if !table.primary_key.columns.contains(&name) || key.contains_key(&name) {
            return Err(SqlError::Unsupported("WHERE must be equality on distinct primary-key columns".into()));
        }
        let column = &table.columns[locate(table, &name)?];
        key.insert(name, coerce(&literal, column)?);
    }
    table.primary_key.columns.iter().map(|name| {
        key.remove(name).ok_or_else(|| SqlError::Resolution("missing primary-key component".into()))
    }).collect()
}

/// SQL engine for one local Bradian root. Table directories are keyed by the
/// persistent TableId, never by user-controlled names. A coarse mutex serializes
/// DDL and DML in this engine instance. This is not MVCC or distributed SQL.
pub struct SqlEngine {
    root: PathBuf,
    catalog: PersistentCatalog,
    operation: Mutex<()>,
    /// ADR 0071 — present when the engine was opened via
    /// `open_with_bace`. Each lazy `TableStore::open_with_bace`
    /// in `table_store()` consults this.
    bace_wiring: Option<BaceWiring>,
    /// ADR 0170 — optional workspace-wide admission cap.
    /// Default `GlobalBudget::default()` is unlimited and
    /// never rejects, so existing callers see no change.
    global_budget: crate::workspace_usage::GlobalBudget,
}

impl SqlEngine {
    pub fn open(root: impl AsRef<Path>) -> SqlResult<Self> {
        Self::open_inner(root.as_ref(), None)
    }

    /// ADR 0071 — open the G14 per-table SQL engine under BACE
    /// wiring. The wiring is captured on the engine so each lazy
    /// `TableStore` open reaches `TableStore::open_with_bace`.
    pub fn open_with_bace(root: impl AsRef<Path>, wiring: BaceWiring) -> SqlResult<Self> {
        Self::open_inner(root.as_ref(), Some(wiring))
    }

    fn open_inner(root: &Path, wiring: Option<BaceWiring>) -> SqlResult<Self> {
        if root.join("history-domain").exists() {
            return Err(SqlError::Unsupported(
                "G23 durable history root requires HistorySqlEngine; legacy SQL ownership refused".into()
            ));
        }
        std::fs::create_dir_all(root)?;
        std::fs::create_dir_all(root.join("tables"))?;
        let catalog = match &wiring {
            Some(w) => PersistentCatalog::open_with_bace(root.join("catalog"), w.clone())?,
            None    => PersistentCatalog::open(root.join("catalog"))?,
        };
        Ok(Self {
            root: root.to_owned(),
            catalog,
            operation: Mutex::new(()),
            bace_wiring: wiring,
            global_budget: crate::workspace_usage::GlobalBudget::default(),
        })
    }

    /// ADR 0170 — attach a workspace-wide admission cap.
    /// Default (no call) leaves `global_budget` at the
    /// unlimited defaults; this method swaps in an
    /// explicit cap that the engine samples before every
    /// statement.
    pub fn with_global_budget(mut self, budget: crate::workspace_usage::GlobalBudget) -> Self {
        self.global_budget = budget;
        self
    }

    /// ADR 0170 — expose the current effective budget so
    /// operators can introspect.
    pub fn global_budget(&self) -> crate::workspace_usage::GlobalBudget {
        self.global_budget
    }

    fn resolve(&self, parts: &[String]) -> SqlResult<Table> {
        qualified(parts, 3)?;
        let snapshot = self.catalog.snapshot();
        let database = snapshot.databases().find(|db| db.name == parts[0])
            .ok_or_else(|| SqlError::Resolution(format!("unknown database {}", parts[0])))?;
        let schema = snapshot.schemas().find(|s| s.database == database.id && s.name == parts[1])
            .ok_or_else(|| SqlError::Resolution(format!("unknown schema {}.{}", parts[0], parts[1])))?;
        let table = snapshot.tables().find(|t| t.schema == schema.id && t.name == parts[2])
            .ok_or_else(|| SqlError::Resolution(format!("unknown table {}", parts.join("."))))?;
        Ok(table.clone())
    }

    fn table_store(&self, table: &Table) -> SqlResult<TableStore> {
        // Only supported schemas reach TableStore; secondary runtime constraints
        // are rejected by its own validator, never silently ignored.
        //
        // ADR 0071 — when the SqlEngine was opened with BACE wiring, each
        // lazy TableStore open threads the wiring through so the per-
        // table RowStore decodes v2 pages and emits v2 on checkpoint.
        let path = self.root.join("tables").join(table.id.0.to_string());
        Ok(match &self.bace_wiring {
            Some(w) => TableStore::open_with_bace(path, table.clone(), w.clone())?,
            None    => TableStore::open(path, table.clone())?,
        })
    }

    /// Execute application-defined typed operations as one durable transaction
    /// on a single SQL-resolved table. A closure error aborts without any WAL
    /// write; successful completion uses one optimistic, synchronized WAL
    /// batch. This is not SQL BEGIN/COMMIT or cross-table transaction support.
    pub fn transact_table<R>(
        &self,
        qualified_table: &str,
        body: impl FnOnce(&mut TableTransaction<'_>) -> SqlResult<R>,
    ) -> SqlResult<(R, u64)> {
        let _guard = self.operation.lock();
        let mut parser = Parser::new(qualified_table)?;
        let name = parser.name()?;
        parser.end()?;
        let table = self.resolve(&name)?;
        let storage = self.table_store(&table)?;
        let mut transaction = storage.begin_transaction()?;
        let result = body(&mut transaction)?;
        let lsn = transaction.commit()?;
        Ok((result, lsn))
    }

    /// Run a SQL transaction session restricted to one resolved table.
    /// BEGIN starts a G13 snapshot; COMMIT publishes a single WAL batch.
    /// If the callback exits without COMMIT, all staged changes are dropped.
    ///
    /// This scoped interface deliberately does not claim cross-table ACID or
    /// independent network-session isolation.
    pub fn with_sql_session<R>(
        &self,
        qualified_table: &str,
        callback: impl FnOnce(&mut SqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        self.with_sql_session_control(qualified_table, ScanControl::default(), callback)
    }

    pub fn with_sql_session_control<R>(
        &self, qualified_table: &str, control: ScanControl,
        callback: impl FnOnce(&mut SqlSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        control.check()?;
        let _guard = self.operation.lock();
        let mut parser = Parser::new(qualified_table)?;
        let name = parser.name()?;
        parser.end()?;
        let table = self.resolve(&name)?;
        let storage = self.table_store(&table)?;
        let mut session = SqlSession {
            control,
            table: &table,
            name,
            storage: &storage,
            transaction: None,
            last_commit_lsn: None,
        };
        callback(&mut session)
    }

    pub fn execute(&self, sql: &str) -> SqlResult<SqlResultSet> {
        self.execute_with_control(sql, &ScanControl::default())
    }

    /// ADR 0100 — prepare a parameterised SQL template.
    /// `?` placeholders are counted and the template is cached
    /// verbatim. Execution substitutes properly-escaped
    /// Value literals into `?` positions, then runs through
    /// `execute`. This is a text-template facility; it is NOT
    /// a parsed-AST plan cache and does NOT change the parser.
    /// Future work can upgrade to prepared plans without a
    /// public API break.
    pub fn prepare(&self, template: &str) -> SqlResult<PreparedStatement> {
        let param_count = count_placeholders(template)?;
        Ok(PreparedStatement { template: template.to_string(), param_count })
    }

    /// ADR 0098 — authz-aware execute. Fast-fails with
    /// `SqlError::Authz` when the context lacks the capability
    /// required by the statement's first keyword. On success,
    /// delegates to `execute`. Unknown keywords pass through
    /// to the parser — authz is a fast-fail LAYER, not the
    /// parser's substitute.
    pub fn execute_as(
        &self,
        authz: &bradian_core::AuthorizationContext,
        sql: &str,
    ) -> SqlResult<SqlResultSet> {
        authz.check_sql(sql)?;
        self.execute(sql)
    }

    /// ADR 0174 — `execute_as` + tamper-evident audit.
    /// Appends one BLAKE3-chained record to `chain` covering
    /// the statement's principal, SQL text and outcome
    /// (success, authz denial or statement error). Preserves
    /// ADR 0098 fast-fail-on-authz-denial behavior: on an
    /// authz rejection the audit record records the
    /// `AuthzDenied(msg)` outcome and then the original
    /// `SqlError::Authz` is returned. On downstream statement
    /// errors the outcome is `Err(msg)` and the original
    /// error is returned. If the audit append itself fails,
    /// the SQL result is preserved and a bubbled
    /// `SqlError::Audit` is returned instead — the caller
    /// chooses whether to retry or escalate; no silent drop.
    pub fn execute_as_audited(
        &self,
        authz: &bradian_core::AuthorizationContext,
        sql: &str,
        chain: &crate::audit_chain::AuditChain,
    ) -> SqlResult<SqlResultSet> {
        use crate::audit_chain::{ChainedAuditEntry, ChainedOutcome};
        let now_ms: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let (result, outcome) = match authz.check_sql(sql) {
            Err(denied) => {
                let outcome = ChainedOutcome::AuthzDenied(denied.to_string());
                (Err(SqlError::Authz(denied)), outcome)
            }
            Ok(()) => match self.execute(sql) {
                Ok(rs) => (Ok(rs), ChainedOutcome::Ok),
                Err(err) => {
                    let msg = err.to_string();
                    (Err(err), ChainedOutcome::Err(msg))
                }
            },
        };
        let entry = ChainedAuditEntry {
            timestamp_ms: now_ms,
            principal_id: authz.principal_id().to_string(),
            sql: sql.to_string(),
            outcome,
        };
        chain.append(&entry).map_err(|e|
            SqlError::Audit(format!("audit chain append failed: {e}")))?;
        result
    }

    /// ADR 0144 — `execute` with a caller-supplied
    /// `BudgetTracker`. The SELECT path charges scanned
    /// rows and checks the wall-clock deadline; breach
    /// returns `SqlError::Budget`. Other paths run
    /// identically to `execute_with_control`.
    pub fn execute_with_budget(
        &self, sql: &str,
        budget: &mut crate::query_budget::BudgetTracker,
    ) -> SqlResult<SqlResultSet> {
        self.execute_impl(sql, &ScanControl::default(), Some(budget), None, None)
    }

    pub fn execute_with_control(&self, sql: &str, control: &ScanControl) -> SqlResult<SqlResultSet> {
        self.execute_impl(sql, control, None, None, None)
    }

    /// ADR 0153 — `execute` with an ADR 0123 row/column/
    /// tenant policy. The SELECT path filters output
    /// rows via `TableAccessPolicy::allows_row` and
    /// projects via `project_row`. Other paths are
    /// unchanged — write authz is left to the caller's
    /// AuthorizationContext (ADR 0098).
    pub fn execute_with_policy(
        &self,
        sql: &str,
        policy: &crate::access_policy::TableAccessPolicy,
        authz: &bradian_core::AuthorizationContext,
    ) -> SqlResult<SqlResultSet> {
        self.execute_impl(sql, &ScanControl::default(), None, Some(policy), Some(authz))
    }

    /// ADR 0156 — `execute` with budget + policy. Both
    /// run against the SELECT path; breach of either
    /// returns the matching `SqlError` variant.
    pub fn execute_with_budget_and_policy(
        &self,
        sql: &str,
        budget: &mut crate::query_budget::BudgetTracker,
        policy: &crate::access_policy::TableAccessPolicy,
        authz: &bradian_core::AuthorizationContext,
    ) -> SqlResult<SqlResultSet> {
        self.execute_impl(sql, &ScanControl::default(),
            Some(budget), Some(policy), Some(authz))
    }

    fn execute_impl(
        &self, sql: &str, control: &ScanControl,
        mut budget: Option<&mut crate::query_budget::BudgetTracker>,
        policy: Option<&crate::access_policy::TableAccessPolicy>,
        authz: Option<&bradian_core::AuthorizationContext>,
    ) -> SqlResult<SqlResultSet> {
        let _guard = self.operation.lock();
        // ADR 0170 — workspace-wide admission cap. Sampled
        // before every statement; unlimited default never
        // rejects.
        let usage = crate::workspace_usage::WorkspaceUsage::for_catalog(&self.catalog);
        self.global_budget.check(&usage)?;
        // ADR 0150 — one iteration = one executed statement.
        if let Some(b) = budget.as_mut() { b.charge_iteration()?; }
        let mut p = Parser::new(sql)?;
        let action = p.word()?;
        match action.as_str() {
            "create" => {
                let kind = p.word()?;
                match kind.as_str() {
                    "database" => {
                        let name = p.name()?;
                        qualified(&name, 1)?;
                        p.end()?;
                        self.catalog.create_database(&name[0])?;
                    }
                    "schema" => {
                        let name = p.name()?;
                        qualified(&name, 2)?;
                        p.end()?;
                        let snapshot = self.catalog.snapshot();
                        let database = snapshot.databases().find(|db| db.name == name[0])
                            .ok_or_else(|| SqlError::Resolution("unknown database".into()))?;
                        self.catalog.create_schema(database.id, &name[1])?;
                    }
                    "table" => {
                        let name = p.name()?;
                        qualified(&name, 3)?;
                        let mut columns = Vec::<Column>::new();
                        let mut primary = Vec::<String>::new();
                        let mut column_collations =
                            std::collections::BTreeMap::new();
                        let mut column_checks =
                            std::collections::BTreeMap::<String, Vec<(crate::model::CheckOp, crate::typed_table::Value)>>::new();
                        let mut unique_keys = Vec::<crate::model::UniqueKey>::new();
                        // ADR 0184 — parser collects inline FK clauses
                        // and the catalog's add_foreign_key is called
                        // after create_table resolves the child id.
                        struct PendingFk {
                            child_cols: Vec<String>,
                            parent_name: Vec<String>,
                            parent_cols: Vec<String>,
                            on_delete: crate::model::ReferentialAction,
                            on_update: crate::model::ReferentialAction,
                        }
                        let mut pending_fks: Vec<PendingFk> = Vec::new();
                        p.symbol('(')?;
                        loop {
                            if p.consume_word("primary") {
                                p.expect("key")?;
                                if !primary.is_empty() {
                                    return Err(SqlError::Syntax("multiple PRIMARY KEY definitions".into()));
                                }
                                primary = p.list(|parser| parser.word())?;
                            } else if p.consume_word("unique") {
                                // ADR 0176 — table-level UNIQUE (col[, ...]).
                                // Constraint name is auto-generated:
                                // "uq_<col1>_<col2>_...".  Catalog
                                // validates column existence; INSERT
                                // path enforces at commit via
                                // UniqueConstraintChecker.
                                let cols = p.list(|parser| parser.word())?;
                                if cols.is_empty() {
                                    return Err(SqlError::Syntax("UNIQUE requires at least one column".into()));
                                }
                                let uq_name = format!("uq_{}", cols.join("_"));
                                unique_keys.push(crate::model::UniqueKey {
                                    name: uq_name, columns: cols,
                                });
                            } else if p.consume_word("foreign") {
                                // ADR 0184 — FOREIGN KEY (child_cols)
                                // REFERENCES <qual_parent> (parent_cols).
                                // ADR 0185 — optional ON DELETE and
                                // ON UPDATE action clauses (CASCADE /
                                // SET NULL / RESTRICT / NO ACTION).
                                p.expect("key")?;
                                let child_cols = p.list(|parser| parser.word())?;
                                p.expect("references")?;
                                let parent_name = p.name()?;
                                qualified(&parent_name, 3)?;
                                let parent_cols = p.list(|parser| parser.word())?;
                                if child_cols.is_empty() || parent_cols.is_empty() {
                                    return Err(SqlError::Syntax("FOREIGN KEY requires at least one column pair".into()));
                                }
                                let mut on_delete = crate::model::ReferentialAction::Restrict;
                                let mut on_update = crate::model::ReferentialAction::Restrict;
                                while p.consume_word("on") {
                                    let which = p.word()?;
                                    let action = parse_referential_action(&mut p)?;
                                    match which.as_str() {
                                        "delete" => on_delete = action,
                                        "update" => on_update = action,
                                        other => return Err(SqlError::Syntax(
                                            format!("expected ON DELETE / ON UPDATE; got ON {other}"))),
                                    }
                                }
                                pending_fks.push(PendingFk {
                                    child_cols, parent_name, parent_cols,
                                    on_delete, on_update,
                                });
                            } else {
                                let column = p.word()?;
                                let data_type = p.data_type()?;
                                let mut nullable = true;
                                if p.consume_word("not") {
                                    p.expect("null")?;
                                    nullable = false;
                                } else {
                                    p.consume_word("null");
                                }
                                // ADR 0175 — optional per-column
                                // COLLATE <name>. Accepts binary
                                // (default) and nocase. Parser
                                // records the collation; the
                                // catalog rejects it on non-text
                                // columns (central definition
                                // check — not silently dropped).
                                if p.consume_word("collate") {
                                    let collation_name = p.word()?;
                                    let collation = match collation_name.as_str() {
                                        "binary" => crate::collation::Collation::Binary,
                                        "nocase" => crate::collation::Collation::CaseInsensitive,
                                        other => return Err(SqlError::Unsupported(
                                            format!("unknown COLLATE {other}; want binary or nocase"))),
                                    };
                                    column_collations.insert(column.clone(), collation);
                                }
                                // ADR 0182 — optional per-column CHECK
                                // clause: `CHECK (<op> <literal>)`.
                                // Multiple CHECK clauses on one column
                                // are AND-joined. Supported ops:
                                // =, <>, <, <=, >, >=.
                                while p.consume_word("check") {
                                    p.symbol('(')?;
                                    let op_sym = p.word_or_symbol_pair_for_check()?;
                                    let literal = p.literal()?;
                                    p.symbol(')')?;
                                    let column_def = Column {
                                        name: column.clone(), data_type: data_type.clone(), nullable,
                                    };
                                    let value = coerce(&literal, &column_def)?;
                                    column_checks.entry(column.clone()).or_default()
                                        .push((op_sym, value));
                                }
                                columns.push(Column { name: column, data_type, nullable });
                            }
                            if !p.consume_symbol(',') { break; }
                        }
                        p.symbol(')')?;
                        p.end()?;
                        if primary.is_empty() {
                            return Err(SqlError::Unsupported("CREATE TABLE requires a PRIMARY KEY".into()));
                        }
                        let snapshot = self.catalog.snapshot();
                        let database = snapshot.databases().find(|db| db.name == name[0])
                            .ok_or_else(|| SqlError::Resolution("unknown database".into()))?;
                        let schema = snapshot.schemas().find(|s| s.database == database.id && s.name == name[1])
                            .ok_or_else(|| SqlError::Resolution("unknown schema".into()))?;
                        let child_id = self.catalog.create_table(schema.id, TableSpec {
                            name: name[2].clone(), columns,
                            primary_key: PrimaryKey { columns: primary },
                            unique_keys,
                            column_collations,
                            column_checks,
                        })?;
                        // ADR 0184 — resolve each FK's parent name
                        // via a fresh catalog snapshot and call
                        // add_foreign_key. The catalog's existing
                        // validation (arity, type match, target is
                        // PK or non-null UNIQUE) handles the rest.
                        if !pending_fks.is_empty() {
                            let snap = self.catalog.snapshot();
                            for (i, fk) in pending_fks.into_iter().enumerate() {
                                let db = snap.databases().find(|d| d.name == fk.parent_name[0])
                                    .ok_or_else(|| SqlError::Resolution(format!("unknown database {}", fk.parent_name[0])))?;
                                let sc = snap.schemas().find(|s| s.database == db.id && s.name == fk.parent_name[1])
                                    .ok_or_else(|| SqlError::Resolution(format!("unknown schema {}.{}", fk.parent_name[0], fk.parent_name[1])))?;
                                let parent = snap.tables().find(|t| t.schema == sc.id && t.name == fk.parent_name[2])
                                    .ok_or_else(|| SqlError::Resolution(format!("unknown table {}.{}.{}",
                                        fk.parent_name[0], fk.parent_name[1], fk.parent_name[2])))?;
                                self.catalog.add_foreign_key(child_id, crate::model::ForeignKeySpec {
                                    name: format!("fk_{}_{i}", name[2]),
                                    columns: fk.child_cols,
                                    referenced_table: parent.id,
                                    referenced_columns: fk.parent_cols,
                                    on_delete: fk.on_delete,
                                    on_update: fk.on_update,
                                })?;
                            }
                        }
                        // TableStore is provisioned on first use. Catalog and
                        // user-table directories are not one atomic DDL commit.
                    }
                    _ => return Err(SqlError::Unsupported(format!("CREATE {kind}"))),
                }
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "insert" => {
                // ADR 0147 — deadline check before mutation.
                if let Some(b) = budget.as_deref() { b.check_deadline()?; }
                p.expect("into")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                let names = p.list(|parser| parser.word())?;
                p.expect("values")?;
                let literals = p.list(|parser| parser.literal())?;
                p.end()?;
                if names.len() != literals.len() || names.len() != table.columns.len() {
                    return Err(SqlError::Unsupported("INSERT must list all table columns once".into()));
                }
                let mut fields = BTreeMap::new();
                for (field, lit) in names.into_iter().zip(literals) {
                    if fields.insert(field.clone(), lit).is_some() {
                        return Err(SqlError::Syntax(format!("repeated INSERT column {field}")));
                    }
                    locate(&table, &field)?;
                }
                let values = table.columns.iter().map(|column| {
                    let literal = fields.remove(&column.name)
                        .ok_or_else(|| SqlError::Resolution(format!("missing column {}", column.name)))?;
                    coerce(&literal, column)
                }).collect::<SqlResult<Vec<_>>>()?;
                // ADR 0151 — preflight the budget BEFORE touching
                // storage. The base SqlEngine has no transaction
                // wrapping, so a post-mutation charge would leave
                // the row committed on budget breach.
                let row_bytes = crate::query_budget::row_bytes(&values);
                if let Some(b) = budget.as_deref() { b.preview_charge(1, row_bytes)?; }
                let store = self.table_store(&table)?;
                // ADR 0182 — evaluate per-column CHECK predicates
                // against the candidate row. No-op when
                // table.column_checks is empty.
                check_row(&table, &values)?;
                // ADR 0184 — FOREIGN KEY parent-exists check
                // for every FK on this table. No-op when
                // table.foreign_keys is empty.
                foreign_key_check(self, &table, &values)?;
                // ADR 0176 — enforce table-level UNIQUE before
                // insert. Scans all existing rows; honest O(N)
                // cost per INSERT until a disk-backed secondary
                // index (ADR 0096 follow-up) lands. No-op when
                // table.unique_keys is empty.
                if !table.unique_keys.is_empty() {
                    let existing = store.scan()?;
                    crate::unique_check::UniqueConstraintChecker::new(&table)
                        .check(&values, existing)?;
                }
                store.insert(&values)?;
                // Commit the charge — guaranteed not to breach.
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(1)?;
                    b.charge_bytes(row_bytes)?;
                }
                Ok(SqlResultSet::Command { affected: 1 })
            }
            "select" => {
                control.check()?;
                // ADR 0144 — deadline check before scan.
                if let Some(b) = budget.as_deref() { b.check_deadline()?; }
                // ADR 0213 — `SELECT COUNT(*) FROM …` short-circuits
                // the projection list. It owns the whole SELECT
                // output; mixing with bare columns requires GROUP
                // BY and is deliberately deferred.
                let count_star = consume_count_star(&mut p)?;
                let caller_wildcard = !count_star && p.consume_symbol('*');
                // ADR 0186 — projection list accepts bare `col`
                // plus `col -> 'key'` / `col ->> 'key'` JSON
                // path extractions.
                let projections = if count_star || caller_wildcard { Vec::new() } else {
                    let mut out = vec![parse_projection(&mut p)?];
                    while p.consume_symbol(',') { out.push(parse_projection(&mut p)?); }
                    out
                };
                p.expect("from")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                let mut projections = if projections.is_empty() {
                    table.columns.iter().map(|c| Projection::Column(c.name.clone())).collect::<Vec<_>>()
                } else { projections };
                if projections.is_empty() {
                    return Err(SqlError::Syntax("SELECT requires columns".into()));
                }
                // ADR 0155 — column-level policy filtering.
                if let Some(p_) = policy {
                    if p_.column_level.is_configured() {
                        if caller_wildcard {
                            projections.retain(|pj| p_.column_level.allows_read(pj.source_column()));
                        } else {
                            for pj in &projections {
                                if !p_.column_level.allows_read(pj.source_column()) {
                                    return Err(SqlError::Policy(
                                        crate::access_policy::PolicyError::ColumnDenied {
                                            column: pj.source_column().into(),
                                        }));
                                }
                            }
                        }
                    }
                }
                let indices: Vec<usize> = projections.iter()
                    .map(|pj| locate(&table, pj.source_column()))
                    .collect::<SqlResult<Vec<_>>>()?;
                // ADR 0190 — WHERE accepts PK equality or
                // JSON-path equality (full-scan post-filter).
                let where_clause = if p.next_is_word("where") {
                    Some(parse_select_where(&mut p, &table)?)
                } else { None };
                // ADR 0212 — ORDER BY / LIMIT / OFFSET on the
                // base engine, reusing the historical window.
                let window = crate::select_window::SelectWindow::parse(&mut p, &table)?;
                p.end()?;
                let scan_control = match window.input_row_limit() {
                    Some(limit) => control.cap_rows(limit),
                    None => control.clone(),
                };
                let store = self.table_store(&table)?;
                let all = match &where_clause {
                    Some(SelectWhere::KeyPredicate(key)) =>
                        store.get(key)?.into_iter().collect::<Vec<_>>(),
                    Some(SelectWhere::JsonPathPredicates(_)) =>
                        store.scan_with_control(&scan_control)?,
                    None => store.scan_with_control(&scan_control)?,
                };
                // ADR 0144 — charge rows then re-check deadline.
                // ADR 0148 — charge the materialized bytes.
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(all.len() as u64)?;
                    b.charge_bytes(crate::query_budget::rows_bytes(&all))?;
                    b.check_deadline()?;
                }
                // ADR 0153 — row/column/tenant policy filter.
                // Applied BEFORE projection so the policy's
                // column names match the full-schema row.
                let all = if let (Some(p), Some(a)) = (policy, authz) {
                    let schema_cols: Vec<String> = table.columns.iter()
                        .map(|c| c.name.clone()).collect();
                    let mut kept = Vec::with_capacity(all.len());
                    for row in all {
                        if p.allows_row(&row, &schema_cols, a)? {
                            kept.push(row);
                        }
                    }
                    kept
                } else { all };
                // ADR 0190 — post-scan filter for JSON-path WHERE.
                let all = match &where_clause {
                    Some(SelectWhere::JsonPathPredicates(preds)) =>
                        apply_where_filter(all, &table, preds)?,
                    _ => all,
                };
                // ADR 0213 — COUNT(*) collapses the filtered scan
                // into a single-row result. ORDER BY / LIMIT /
                // OFFSET are discarded: counting is scalar.
                if count_star {
                    let total = all.len() as i64;
                    return Ok(SqlResultSet::Rows {
                        columns: vec!["count".into()],
                        rows: vec![vec![Value::Int64(total)]],
                    });
                }
                // ADR 0212 — apply ORDER BY / LIMIT / OFFSET
                // before projection so sort keys can reference
                // columns that are not in the projection list.
                let all = window.apply(all, &table)?;
                let base_rows = crate::typed_table::project_rows_with_control(all, &indices, control)?;
                // ADR 0186 — apply JSON path extraction per cell
                // after the base projection. Bare columns pass
                // through unchanged.
                let rows: Vec<Vec<Value>> = base_rows.into_iter().map(|row|
                    row.into_iter().zip(&projections).map(|(value, pj)| apply_projection(pj, value))
                        .collect::<SqlResult<Vec<Value>>>()
                ).collect::<SqlResult<Vec<Vec<Value>>>>()?;
                let columns: Vec<String> = projections.iter().map(|pj| pj.label()).collect();
                Ok(SqlResultSet::Rows { columns, rows })
            }
            "update" => {
                let name = p.name()?;
                let table = self.resolve(&name)?;
                p.expect("set")?;
                // ADR 0192 — accept bare-column and JSON-path
                // assignment LHS forms.
                let changes = p.path_assignments()?;
                // ADR 0199 — WHERE accepts PK equality OR a
                // JSON-path predicate (full-scan + filter).
                let where_clause = parse_select_where(&mut p, &table)?;
                p.end()?;
                let mut column_assignments: BTreeMap<usize, Value> = BTreeMap::new();
                let mut json_assignments: Vec<(usize, Vec<JsonSegment>, bool, Literal)> = Vec::new();
                let mut json_removes: Vec<(usize, Vec<JsonSegment>)> = Vec::new();
                let mut json_inserts: Vec<(usize, Vec<JsonSegment>, bool, Literal)> = Vec::new();
                let mut json_concats: Vec<(usize, Vec<JsonSegment>, Literal)> = Vec::new();
                for change in changes {
                    match change {
                        SetAssignment::Column { column, value } => {
                            let position = locate(&table, &column)?;
                            if table.primary_key.columns.contains(&column) {
                                return Err(SqlError::Unsupported("UPDATE cannot modify primary-key columns".into()));
                            }
                            if column_assignments.insert(position, coerce(&value, &table.columns[position])?).is_some() {
                                return Err(SqlError::Syntax("duplicate SET column".into()));
                            }
                        }
                        SetAssignment::JsonPath { column, path, as_text, value } => {
                            let position = locate(&table, &column)?;
                            if table.primary_key.columns.contains(&column) {
                                return Err(SqlError::Unsupported("UPDATE cannot modify primary-key columns".into()));
                            }
                            json_assignments.push((position, path, as_text, value));
                        }
                        SetAssignment::JsonRemove { column, path } => {
                            if table.primary_key.columns.contains(&column) {
                                return Err(SqlError::Unsupported("UPDATE cannot modify primary-key columns".into()));
                            }
                            let position = locate(&table, &column)?;
                            json_removes.push((position, path));
                        }
                        SetAssignment::JsonInsert { column, path, as_text, value } => {
                            if table.primary_key.columns.contains(&column) {
                                return Err(SqlError::Unsupported("UPDATE cannot modify primary-key columns".into()));
                            }
                            let position = locate(&table, &column)?;
                            json_inserts.push((position, path, as_text, value));
                        }
                        SetAssignment::JsonConcat { column, path, value } => {
                            if table.primary_key.columns.contains(&column) {
                                return Err(SqlError::Unsupported("UPDATE cannot modify primary-key columns".into()));
                            }
                            let position = locate(&table, &column)?;
                            json_concats.push((position, path, value));
                        }
                    }
                }
                // ADR 0147 — deadline check before mutation.
                if let Some(b) = budget.as_deref() { b.check_deadline()?; }
                let store = self.table_store(&table)?;
                // ADR 0199 — select target rows. PK equality
                // stays on the fast lookup path; JSON path
                // scans the table and filters per row.
                let pk_positions: Vec<usize> = table.primary_key.columns.iter()
                    .map(|c| locate(&table, c))
                    .collect::<SqlResult<Vec<_>>>()?;
                let targets: Vec<(Vec<Value>, Vec<Value>)> = match &where_clause {
                    SelectWhere::KeyPredicate(pk) => match store.get(pk)? {
                        Some(row) => vec![(pk.clone(), row)],
                        None => Vec::new(),
                    },
                    SelectWhere::JsonPathPredicates(preds) => {
                        let filtered = apply_where_filter(store.scan()?, &table, preds)?;
                        filtered.into_iter().map(|row| {
                            let pk: Vec<Value> = pk_positions.iter()
                                .map(|p| row[*p].clone()).collect();
                            (pk, row)
                        }).collect()
                    }
                };
                let mut written_bytes: u64 = 0;
                let mut affected: u64 = 0;
                for (pk, mut row) in targets {
                    for (pos, value) in &column_assignments {
                        row[*pos] = value.clone();
                    }
                    for (pos, path, as_text, literal) in &json_assignments {
                        let col_name = table.columns[*pos].name.clone();
                        row[*pos] = apply_json_path_mutation(
                            &col_name, &row[*pos], path, *as_text, literal)?;
                    }
                    // ADR 0200 — JSON removes after mutations.
                    for (pos, path) in &json_removes {
                        let col_name = table.columns[*pos].name.clone();
                        row[*pos] = apply_json_path_remove(
                            &col_name, &row[*pos], path)?;
                    }
                    // ADR 0201 — splice inserts last, so a
                    // preceding REMOVE at the same index doesn't
                    // step on the INSERT.
                    for (pos, path, as_text, literal) in &json_inserts {
                        let col_name = table.columns[*pos].name.clone();
                        row[*pos] = apply_json_path_insert(
                            &col_name, &row[*pos], path, *as_text, literal)?;
                    }
                    // ADR 0203 — CONCAT after splice so appended
                    // elements land on the already-spliced array.
                    for (pos, path, literal) in &json_concats {
                        let col_name = table.columns[*pos].name.clone();
                        row[*pos] = apply_json_path_concat(
                            &col_name, &row[*pos], path, literal)?;
                    }
                    check_row(&table, &row)?;
                    foreign_key_check(self, &table, &row)?;
                    if !table.unique_keys.is_empty() {
                        let others = store.scan()?.into_iter()
                            .filter(|r| pk_positions.iter()
                                .zip(pk.iter())
                                .any(|(p_, k)| &r[*p_] != k));
                        crate::unique_check::UniqueConstraintChecker::new(&table)
                            .check(&row, others)?;
                    }
                    let row_bytes = crate::query_budget::row_bytes(&row);
                    written_bytes = written_bytes.saturating_add(row_bytes);
                    if let Some(b) = budget.as_deref() {
                        b.preview_charge(1, row_bytes)?;
                    }
                    store.replace(&row)?;
                    affected += 1;
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(affected)?;
                    if affected > 0 { b.charge_bytes(written_bytes)?; }
                }
                Ok(SqlResultSet::Command { affected })
            }
            "delete" => {
                // ADR 0147 — deadline check before mutation.
                if let Some(b) = budget.as_deref() { b.check_deadline()?; }
                p.expect("from")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                // ADR 0199 — WHERE accepts PK equality OR a
                // JSON-path predicate (full-scan + filter).
                let where_clause = parse_select_where(&mut p, &table)?;
                p.end()?;
                let store = self.table_store(&table)?;
                let pk_positions: Vec<usize> = table.primary_key.columns.iter()
                    .map(|c| locate(&table, c))
                    .collect::<SqlResult<Vec<_>>>()?;
                let targets: Vec<(Vec<Value>, Vec<Value>)> = match &where_clause {
                    SelectWhere::KeyPredicate(pk) => match store.get(pk)? {
                        Some(row) => vec![(pk.clone(), row)],
                        None => Vec::new(),
                    },
                    SelectWhere::JsonPathPredicates(preds) => {
                        let filtered = apply_where_filter(store.scan()?, &table, preds)?;
                        filtered.into_iter().map(|row| {
                            let pk: Vec<Value> = pk_positions.iter()
                                .map(|p| row[*p].clone()).collect();
                            (pk, row)
                        }).collect()
                    }
                };
                let mut deleted_bytes: u64 = 0;
                let mut affected: u64 = 0;
                for (pk, row) in targets {
                    let row_bytes = crate::query_budget::row_bytes(&row);
                    if let Some(b) = budget.as_deref() {
                        b.preview_charge(1, row_bytes)?;
                    }
                    foreign_key_restrict(self, &table, &pk)?;
                    store.delete(&pk)?;
                    deleted_bytes = deleted_bytes.saturating_add(row_bytes);
                    affected += 1;
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(affected)?;
                    if affected > 0 { b.charge_bytes(deleted_bytes)?; }
                }
                Ok(SqlResultSet::Command { affected })
            }
            // ADR 0113 — TRUNCATE TABLE removes every row from
            // ONE table without touching catalog schema.
            // Scans PK list, deletes each. No cascading.
            "truncate" => {
                // ADR 0147 — deadline check before scan + bulk delete.
                if let Some(b) = budget.as_deref() { b.check_deadline()?; }
                p.expect("table")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                p.end()?;
                let store = self.table_store(&table)?;
                let rows = store.scan()?;
                // ADR 0149 — charge bytes once for the whole truncated set.
                let truncated_bytes = crate::query_budget::rows_bytes(&rows);
                // ADR 0151 — preflight before bulk delete.
                if let Some(b) = budget.as_deref() {
                    b.preview_charge(rows.len() as u64, truncated_bytes)?;
                }
                let mut affected = 0u64;
                for row in rows {
                    let pk_values: Vec<Value> = table.primary_key.columns.iter()
                        .map(|c| {
                            let idx = table.columns.iter()
                                .position(|col| col.name == *c).unwrap();
                            row[idx].clone()
                        }).collect();
                    store.delete(&pk_values)?;
                    affected += 1;
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(affected)?;
                    b.charge_bytes(truncated_bytes)?;
                }
                Ok(SqlResultSet::Command { affected })
            }
            // ADR 0165/0166 — ALTER TABLE statements.
            //   ALTER TABLE <name> RENAME TO <new_name>
            //   ALTER TABLE <name> RENAME COLUMN <old> TO <new>
            //   ALTER TABLE <name> DROP COLUMN <name>
            //   ALTER TABLE <name> ALTER COLUMN <col> SET DEFAULT <literal>
            //   ALTER TABLE <name> ALTER COLUMN <col> DROP DEFAULT
            //   ALTER TABLE <name> ALTER COLUMN <col> TYPE <type>
            "alter" => {
                p.expect("table")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                if p.consume_word("rename") {
                    if p.consume_word("to") {
                        let new_name = p.word()?;
                        p.end()?;
                        self.catalog.rename_table(table.id, &new_name)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
                    p.expect("column")?;
                    let old = p.word()?;
                    p.expect("to")?;
                    let new = p.word()?;
                    p.end()?;
                    self.catalog.rename_column(table.id, &old, &new)?;
                    return Ok(SqlResultSet::Command { affected: 0 });
                }
                if p.consume_word("drop") {
                    p.expect("column")?;
                    let col = p.word()?;
                    p.end()?;
                    self.catalog.drop_column(table.id, &col)?;
                    return Ok(SqlResultSet::Command { affected: 0 });
                }
                if p.consume_word("alter") {
                    p.expect("column")?;
                    let col = p.word()?;
                    if p.consume_word("set") {
                        p.expect("default")?;
                        let column = table.columns.iter()
                            .find(|c| c.name == col)
                            .ok_or_else(|| SqlError::Resolution(
                                format!("unknown column {col}")))?;
                        let literal = p.literal()?;
                        let value = coerce(&literal, column)?;
                        p.end()?;
                        self.catalog.set_column_default(table.id, &col, value)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
                    if p.consume_word("drop") {
                        p.expect("default")?;
                        p.end()?;
                        self.catalog.drop_column_default(table.id, &col)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
                    if p.consume_word("type") {
                        let new_type = p.data_type()?;
                        p.end()?;
                        self.catalog.widen_column_type(table.id, &col, new_type)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
                    return Err(SqlError::Syntax(
                        "ALTER COLUMN expects SET DEFAULT / DROP DEFAULT / TYPE".into()));
                }
                Err(SqlError::Syntax(
                    "ALTER TABLE expects RENAME / DROP COLUMN / ALTER COLUMN".into()))
            }
            _ => Err(SqlError::Unsupported(format!("statement {action}"))),
        }
    }
}


/// A scoped SQL transaction session for one fully qualified native table.
/// BEGIN creates a consistent G13 snapshot; DML is staged until COMMIT.
/// Dropping a session with an open transaction discards its staged changes.
pub struct SqlSession<'a> {
    control: ScanControl,
    table: &'a Table,
    name: Vec<String>,
    storage: &'a TableStore,
    transaction: Option<TableTransaction<'a>>,
    last_commit_lsn: Option<u64>,
}

impl<'a> SqlSession<'a> {
    pub fn in_transaction(&self) -> bool { self.transaction.is_some() }
    pub fn last_commit_lsn(&self) -> Option<u64> { self.last_commit_lsn }

    pub fn execute(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        // A statement error aborts the current unit of work. This avoids
        // accidentally committing earlier writes after a failed SQL command.
        let result = self.execute_inner(sql);
        if result.is_err() {
            self.transaction = None;
        }
        result
    }

    fn execute_inner(&mut self, sql: &str) -> SqlResult<SqlResultSet> {
        let mut p = Parser::new(sql)?;
        let command = p.word()?;
        match command.as_str() {
            "begin" => {
                p.consume_word("transaction");
                p.end()?;
                if self.transaction.is_some() {
                    return Err(SqlError::Unsupported("nested BEGIN is not supported".into()));
                }
                let transaction: TableTransaction<'a> = self.storage.begin_transaction_with_control(&self.control)?;
                self.transaction = Some(transaction);
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "commit" => {
                p.end()?;
                let tx = self.transaction.take()
                    .ok_or_else(|| SqlError::Unsupported("COMMIT requires an active transaction".into()))?;
                self.last_commit_lsn = Some(tx.commit()?);
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "rollback" => {
                p.end()?;
                let tx = self.transaction.take()
                    .ok_or_else(|| SqlError::Unsupported("ROLLBACK requires an active transaction".into()))?;
                tx.rollback();
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "insert" | "select" | "update" | "delete" => {
                let tx = self.transaction.as_mut()
                    .ok_or_else(|| SqlError::Unsupported("BEGIN is required before transactional SQL".into()))?;
                match command.as_str() {
                    "insert" => {
                        p.expect("into")?;
                        let name = p.name()?;
                        if name != self.name {
                            return Err(SqlError::Unsupported("cross-table transaction is unsupported".into()));
                        }
                        let columns = p.list(|parser| parser.word())?;
                        p.expect("values")?;
                        let values = p.list(|parser| parser.literal())?;
                        p.end()?;
                        if columns.len() != values.len() || columns.len() != self.table.columns.len() {
                            return Err(SqlError::Unsupported("INSERT must list every table column once".into()));
                        }
                        let mut mapped = BTreeMap::new();
                        for (name, literal) in columns.into_iter().zip(values) {
                            locate(self.table, &name)?;
                            if mapped.insert(name.clone(), literal).is_some() {
                                return Err(SqlError::Syntax(format!("repeated INSERT column {name}")));
                            }
                        }
                        let row = self.table.columns.iter().map(|col| {
                            let literal = mapped.remove(&col.name)
                                .ok_or_else(|| SqlError::Resolution(format!("missing column {}", col.name)))?;
                            coerce(&literal, col)
                        }).collect::<SqlResult<Vec<_>>>()?;
                        tx.insert(&row)?;
                        Ok(SqlResultSet::Command { affected: 1 })
                    }
                    "select" => {
                        self.control.check()?;
                        let projected = if p.consume_symbol('*') {
                            self.table.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
                        } else {
                            let mut columns = vec![p.word()?];
                            while p.consume_symbol(',') { columns.push(p.word()?); }
                            columns
                        };
                        let positions = projected.iter().map(|c| locate(self.table,c))
                            .collect::<SqlResult<Vec<_>>>()?;
                        p.expect("from")?;
                        if p.name()? != self.name {
                            return Err(SqlError::Unsupported("cross-table transaction is unsupported".into()));
                        }
                        let key = if p.next_is_word("where") { Some(key_predicate(&mut p, self.table)?) } else { None };
                        p.end()?;
                        let rows = if let Some(key) = key { tx.get(&key)?.into_iter().collect::<Vec<_>>() }
                            else { tx.scan_with_control(&self.control)? };
                        Ok(SqlResultSet::Rows {
                            columns: projected,
                            rows: crate::typed_table::project_rows_with_control(rows, &positions, &self.control)?,
                        })
                    }
                    "update" => {
                        if p.name()? != self.name {
                            return Err(SqlError::Unsupported("cross-table transaction is unsupported".into()));
                        }
                        p.expect("set")?;
                        let updates = p.assignments()?;
                        let key = key_predicate(&mut p, self.table)?;
                        p.end()?;
                        let mut replacements = BTreeMap::new();
                        for (name, literal) in updates {
                            if self.table.primary_key.columns.contains(&name) {
                                return Err(SqlError::Unsupported("primary-key UPDATE unsupported".into()));
                            }
                            let index = locate(self.table, &name)?;
                            if replacements.insert(index, coerce(&literal, &self.table.columns[index])?).is_some() {
                                return Err(SqlError::Syntax("duplicate SET column".into()));
                            }
                        }
                        let affected = if let Some(mut row) = tx.get(&key)? {
                            for (index, value) in replacements { row[index] = value; }
                            tx.replace(&row)?;
                            1
                        } else { 0 };
                        Ok(SqlResultSet::Command { affected })
                    }
                    "delete" => {
                        p.expect("from")?;
                        if p.name()? != self.name {
                            return Err(SqlError::Unsupported("cross-table transaction is unsupported".into()));
                        }
                        let key = key_predicate(&mut p, self.table)?;
                        p.end()?;
                        let affected = if tx.get(&key)?.is_some() {
                            tx.delete(&key)?;
                            1
                        } else { 0 };
                        Ok(SqlResultSet::Command { affected })
                    }
                    _ => unreachable!(),
                }
            }
            _ => Err(SqlError::Unsupported(format!(
                "transaction sessions do not support {command}"
            ))),
        }
    }
}

/// ADR 0100 — prepared statement handle produced by
/// `SqlEngine::prepare`. Holds a SQL template with `?`
/// placeholders and the count of placeholders. Execute by
/// binding a slice of `Value`s and running against a
/// `SqlEngine`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedStatement {
    template: String,
    param_count: usize,
}

impl PreparedStatement {
    pub fn template(&self) -> &str { &self.template }
    pub fn param_count(&self) -> usize { self.param_count }

    /// Substitute placeholders with escaped literals and
    /// execute against the given engine. Returns
    /// `SqlError::Unsupported` on parameter-count mismatch.
    pub fn execute(&self, engine: &SqlEngine, params: &[Value]) -> SqlResult<SqlResultSet> {
        let sql = self.bind(params)?;
        engine.execute(&sql)
    }

    /// Substitute placeholders and execute under an authz
    /// context (ADR 0098).
    pub fn execute_as(
        &self,
        engine: &SqlEngine,
        authz: &bradian_core::AuthorizationContext,
        params: &[Value],
    ) -> SqlResult<SqlResultSet> {
        let sql = self.bind(params)?;
        engine.execute_as(authz, &sql)
    }

    /// Substitute placeholders; expose the materialised SQL for
    /// callers that want to inspect or log it.
    pub fn bind(&self, params: &[Value]) -> SqlResult<String> {
        if params.len() != self.param_count {
            return Err(SqlError::Unsupported(format!(
                "prepared statement needs {} parameter(s), got {}",
                self.param_count, params.len(),
            )));
        }
        let mut out = String::with_capacity(self.template.len());
        let mut iter = params.iter();
        let chars = self.template.chars();
        let mut in_string = false;
        for c in chars {
            if c == '\'' {
                in_string = !in_string;
                out.push(c);
            } else if c == '?' && !in_string {
                let p = iter.next().ok_or_else(||
                    SqlError::Unsupported("prepared statement param iterator exhausted".into())
                )?;
                out.push_str(&value_to_sql_literal(p));
            } else {
                out.push(c);
            }
        }
        Ok(out)
    }
}

/// Count `?` placeholders outside of string literals. Returns
/// `SqlError::Syntax` on unterminated string literals.
fn count_placeholders(sql: &str) -> SqlResult<usize> {
    let mut count = 0usize;
    let mut in_string = false;
    for c in sql.chars() {
        if c == '\'' {
            in_string = !in_string;
        } else if c == '?' && !in_string {
            count += 1;
        }
    }
    if in_string {
        return Err(SqlError::Syntax(
            "unterminated string literal in prepared-statement template".into(),
        ));
    }
    Ok(count)
}

/// Render a `Value` as a SQL literal for prepared-statement
/// substitution. Strings are single-quoted with embedded `'`
/// doubled per SQL's standard escape. NULL renders as `NULL`.
fn value_to_sql_literal(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Int32(n) => n.to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Decimal(n) => n.to_string(),
        Value::String(s) | Value::Json(s) => format!("'{}'", s.replace('\'', "''")),
        Value::TimestampTz(micros) => micros.to_string(),
        Value::Uuid(bytes) => {
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            format!("'{hex}'")
        }
    }
}
