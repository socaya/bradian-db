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
    /// ADR 0249 — user catalog mutation failed (invalid name,
    /// duplicate / not found, empty password, journal error).
    #[error(transparent)]
    User(#[from] crate::persistent_user_catalog::PersistentUserCatalogError),
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
        } else if ch.is_ascii_digit() {
            // ADR 0239 — numeric literals never include a
            // leading sign at the lexer level; `-5` emerges as
            // `Symbol('-'), Number("5")` which `Parser::literal`
            // collapses into `Literal::Number("-5")` while
            // still letting the arithmetic parser see
            // `col - 5` as subtraction.
            let mut number = String::new();
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
        } else if "(),.=;*<>:+-/%".contains(ch) {
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

/// ADR 0213 + ADR 0214 — scalar aggregate at the head of a
/// SELECT. `COUNT(*)` counts filtered rows; `MIN(col)`,
/// `MAX(col)`, `SUM(col)` iterate the filtered scan and
/// accumulate. Each aggregate owns the whole projection list.
#[derive(Clone, Debug)]
pub(crate) enum AggSpec {
    CountStar,
    Min(String),
    Max(String),
    Sum(String),
    /// ADR 0215 — arithmetic mean over non-null values. Int32
    /// / Int64 columns yield a truncated Int64; Decimal
    /// columns preserve the declared scale (half-even
    /// rounding on the final divide).
    Avg(String),
    /// ADR 0216 — non-null count of one column.
    CountCol(String),
    /// ADR 0216 — distinct non-null count of one column.
    CountDistinct(String),
    /// ADR 0237 — aggregate over a scalar expression.
    MinExpr(FuncExpr),
    MaxExpr(FuncExpr),
    SumExpr(FuncExpr),
    AvgExpr(FuncExpr),
    CountColExpr(FuncExpr),
    CountDistinctExpr(FuncExpr),
}

impl AggSpec {
    pub(crate) fn result_column(&self) -> &'static str {
        match self {
            AggSpec::CountStar => "count",
            AggSpec::Min(_) | AggSpec::MinExpr(_) => "min",
            AggSpec::Max(_) | AggSpec::MaxExpr(_) => "max",
            AggSpec::Sum(_) | AggSpec::SumExpr(_) => "sum",
            AggSpec::Avg(_) | AggSpec::AvgExpr(_) => "avg",
            AggSpec::CountCol(_) | AggSpec::CountDistinct(_)
                | AggSpec::CountColExpr(_) | AggSpec::CountDistinctExpr(_) => "count",
        }
    }
}

/// ADR 0223 — ON CONFLICT (pk_cols) DO NOTHING | DO UPDATE SET …
/// The conflict target is scoped to the primary-key column set for
/// the MVP; UNIQUE-key targets are a future extension.
#[derive(Clone, Debug)]
pub(crate) enum OnConflict {
    DoNothing,
    DoUpdate { assignments: Vec<(String, Literal)> },
}

pub(crate) fn parse_on_conflict(
    p: &mut Parser, table: &Table,
) -> SqlResult<Option<OnConflict>> {
    if !p.consume_word("on") { return Ok(None); }
    p.expect("conflict")?;
    let cols = p.list(|parser| parser.word())?;
    // Conflict target must be the PK column set (order-free).
    let mut target_sorted = cols.clone();
    target_sorted.sort();
    let mut pk_sorted = table.primary_key.columns.clone();
    pk_sorted.sort();
    if target_sorted != pk_sorted {
        return Err(SqlError::Unsupported(
            "ON CONFLICT target must be the primary-key column set".into()));
    }
    p.expect("do")?;
    if p.consume_word("nothing") { return Ok(Some(OnConflict::DoNothing)); }
    p.expect("update")?;
    p.expect("set")?;
    let mut assignments: Vec<(String, Literal)> = Vec::new();
    loop {
        let col = p.word()?;
        if table.primary_key.columns.contains(&col) {
            return Err(SqlError::Unsupported(
                "ON CONFLICT DO UPDATE cannot modify primary-key columns".into()));
        }
        locate(table, &col)?;
        p.symbol('=')?;
        assignments.push((col, p.literal()?));
        if !p.consume_symbol(',') { break; }
    }
    Ok(Some(OnConflict::DoUpdate { assignments }))
}

/// ADR 0221 — optional `RETURNING col[, col] | *` tail on
/// INSERT / UPDATE / DELETE. Returns `None` when absent; `Some(vec)`
/// with a list of `Projection`s otherwise. A bare `*` expands to
/// every table column.
pub(crate) fn parse_returning(
    p: &mut Parser, table: &Table,
) -> SqlResult<Option<Vec<Projection>>> {
    if !p.consume_word("returning") { return Ok(None); }
    if p.consume_symbol('*') {
        return Ok(Some(table.columns.iter()
            .map(|c| Projection::Column(c.name.clone())).collect()));
    }
    let mut items = vec![parse_projection(p)?];
    while p.consume_symbol(',') { items.push(parse_projection(p)?); }
    for item in &items { locate(table, item.source_column())?; }
    Ok(Some(items))
}

/// ADR 0221 — turn captured full-schema rows from a DML
/// statement into the appropriate `SqlResultSet`. When
/// `projections` is `None`, return the legacy
/// `Command { affected }` shape; otherwise apply each
/// projection per captured row and return `Rows { columns, rows }`.
pub(crate) fn finish_dml_result_with_table(
    table: &Table,
    captured: Vec<Vec<Value>>,
    projections: Option<&[Projection]>,
) -> SqlResult<SqlResultSet> {
    let affected = captured.len() as u64;
    match projections {
        None => Ok(SqlResultSet::Command { affected }),
        Some(projs) => {
            let positions: Vec<usize> = projs.iter()
                .map(|pj| locate(table, pj.source_column()))
                .collect::<SqlResult<Vec<_>>>()?;
            let columns: Vec<String> = projs.iter().map(|pj| pj.label()).collect();
            let rows: Vec<Vec<Value>> = captured.into_iter().map(|row| {
                positions.iter().zip(projs).map(|(pos, pj)|
                    apply_projection(pj, row[*pos].clone())
                ).collect::<SqlResult<Vec<Value>>>()
            }).collect::<SqlResult<Vec<Vec<Value>>>>()?;
            Ok(SqlResultSet::Rows { columns, rows })
        }
    }
}

/// ADR 0229 — find the next top-level `IN (SELECT` (or
/// `NOT IN (SELECT`) match in the SQL respecting single-quoted
/// strings. Returns `Some((in_start, paren_open, select_start))`
/// pointing at the `IN` keyword, the `(`, and the `S` of
/// `SELECT`, or `None` when no match remains.
fn find_in_subquery(sql: &[u8], from: usize) -> Option<(usize, usize)> {
    let word_boundary = |b: u8| !b.is_ascii_alphanumeric() && b != b'_';
    let mut i = from;
    let mut in_quote = false;
    while i < sql.len() {
        let b = sql[i];
        if in_quote {
            if b == b'\'' { in_quote = false; }
            i += 1; continue;
        }
        if b == b'\'' { in_quote = true; i += 1; continue; }
        // Look for a word " IN " (case-insensitive).
        if (i == 0 || word_boundary(sql[i - 1]))
            && i + 2 <= sql.len()
            && sql[i..i + 2].eq_ignore_ascii_case(b"IN")
            && (i + 2 == sql.len() || word_boundary(sql[i + 2]))
        {
            let mut after = i + 2;
            while after < sql.len() && (sql[after] == b' ' || sql[after] == b'\t' || sql[after] == b'\n') { after += 1; }
            if after < sql.len() && sql[after] == b'(' {
                let mut k = after + 1;
                while k < sql.len() && (sql[k] == b' ' || sql[k] == b'\t' || sql[k] == b'\n') { k += 1; }
                if k + 6 <= sql.len()
                    && sql[k..k + 6].eq_ignore_ascii_case(b"SELECT")
                    && (k + 6 == sql.len() || word_boundary(sql[k + 6]))
                {
                    return Some((after, k));
                }
            }
        }
        i += 1;
    }
    None
}

/// ADR 0229 — find the position of the paren that closes the
/// one at `open` (which must point AT the `(`).  Returns `None`
/// when unbalanced.
fn find_matching_paren(sql: &[u8], open: usize) -> Option<usize> {
    assert_eq!(sql[open], b'(');
    let mut depth = 0i32;
    let mut i = open;
    let mut in_quote = false;
    while i < sql.len() {
        let b = sql[i];
        if in_quote {
            if b == b'\'' { in_quote = false; }
            i += 1; continue;
        }
        match b {
            b'\'' => in_quote = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 { return Some(i); }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// ADR 0229 — serialize a single column of values as a SQL
/// literal list suitable for an `IN (...)` RHS. Int / Text /
/// Null supported; other scalar types fail with `Type`.
fn format_in_list(values: &[Value]) -> SqlResult<String> {
    let mut parts: Vec<String> = Vec::with_capacity(values.len());
    for v in values {
        parts.push(match v {
            Value::Null => "NULL".to_string(),
            Value::Int32(n) => n.to_string(),
            Value::Int64(n) => n.to_string(),
            Value::Decimal(n) => n.to_string(),
            Value::Boolean(b) => if *b { "true".into() } else { "false".into() },
            Value::String(s) => format!("'{}'", s.replace('\'', "''")),
            Value::Json(s) => format!("'{}'", s.replace('\'', "''")),
            Value::Uuid(_) | Value::TimestampTz(_) => return Err(SqlError::Type(
                "IN (SELECT …) subquery column type not representable as a literal".into())),
        });
    }
    Ok(parts.join(","))
}

/// ADR 0225 — split a top-level `SELECT … UNION [ALL] SELECT …`
/// chain into its parts. Returns `None` when the input has no
/// top-level UNION; `Some(parts)` otherwise. Each part is a
/// tuple of `(preceded_by_union_all, sql_fragment)`. The first
/// part's boolean is always `false` (no preceding separator).
pub(crate) fn split_top_level_unions(sql: &str) -> Option<Vec<(bool, String)>> {
    let bytes = sql.as_bytes();
    let mut separators: Vec<(bool, usize, usize)> = Vec::new();
    let mut i = 0;
    let mut in_quote = false;
    let mut paren_depth = 0u32;
    let word_boundary = |b: u8| !b.is_ascii_alphanumeric() && b != b'_';
    while i < bytes.len() {
        let b = bytes[i];
        if in_quote {
            if b == b'\'' { in_quote = false; }
            i += 1; continue;
        }
        match b {
            b'\'' => { in_quote = true; i += 1; continue; }
            b'(' => { paren_depth += 1; i += 1; continue; }
            b')' => { paren_depth = paren_depth.saturating_sub(1); i += 1; continue; }
            _ => {}
        }
        if paren_depth == 0
            && (i == 0 || word_boundary(bytes[i - 1]))
            && i + 5 <= bytes.len()
            && bytes[i..i + 5].eq_ignore_ascii_case(b"UNION")
            && (i + 5 == bytes.len() || word_boundary(bytes[i + 5]))
        {
            let mut after = i + 5;
            while after < bytes.len() && (bytes[after] == b' ' || bytes[after] == b'\t' || bytes[after] == b'\n') {
                after += 1;
            }
            let mut all = false;
            if after + 3 <= bytes.len()
                && bytes[after..after + 3].eq_ignore_ascii_case(b"ALL")
                && (after + 3 == bytes.len() || word_boundary(bytes[after + 3]))
            {
                all = true;
                after += 3;
            }
            separators.push((all, i, after));
            i = after;
            continue;
        }
        i += 1;
    }
    if separators.is_empty() { return None; }
    let mut cursor = 0usize;
    let mut out: Vec<(bool, String)> = Vec::new();
    let mut pending_all = false;
    for (all, start, end) in separators {
        out.push((pending_all, sql[cursor..start].trim().to_string()));
        pending_all = all;
        cursor = end;
    }
    out.push((pending_all, sql[cursor..].trim().to_string()));
    Some(out)
}

/// ADR 0219 — dedup a slice of row tuples using the same
/// cross-type equality (`compare_values_for_check`) that GROUP
/// BY and COUNT(DISTINCT) use. Preserves first-seen order for
/// callers that then sort deterministically.
pub(crate) fn dedup_rows(rows: Vec<Vec<Value>>) -> Vec<Vec<Value>> {
    let mut out: Vec<Vec<Value>> = Vec::with_capacity(rows.len());
    for row in rows {
        let duplicate = out.iter().any(|existing|
            existing.len() == row.len()
            && existing.iter().zip(row.iter()).all(|(a, b)|
                compare_values_for_check(a, b) == std::cmp::Ordering::Equal));
        if !duplicate { out.push(row); }
    }
    out
}

/// ADR 0219 — sort the deduped DISTINCT output in ascending
/// row-tuple order so callers get deterministic results without
/// writing ORDER BY. NULL sorts last at each position, matching
/// the GROUP BY convention from ADR 0217.
pub(crate) fn sort_distinct_output(rows: &mut [Vec<Value>]) {
    rows.sort_by(|a, b| {
        for (lhs, rhs) in a.iter().zip(b.iter()) {
            match (lhs, rhs) {
                (Value::Null, Value::Null) => continue,
                (Value::Null, _) => return std::cmp::Ordering::Greater,
                (_, Value::Null) => return std::cmp::Ordering::Less,
                (l, r) => {
                    let ord = compare_values_for_check(l, r);
                    if ord != std::cmp::Ordering::Equal { return ord; }
                }
            }
        }
        std::cmp::Ordering::Equal
    });
}

/// ADR 0232 + 0234 — argument to a scalar function call.
/// Columns are stored as raw name (dotted for joined queries);
/// literals materialize at parse time; nested wraps another
/// scalar function expression so `UPPER(TRIM(name))` composes.
#[derive(Clone, Debug)]
pub(crate) enum FuncArg {
    ColumnRef(String),
    Literal(Value),
    Nested(Box<FuncExpr>),
}

/// ADR 0233 + 0238 — one `WHEN predicate THEN value` arm of a CASE.
#[derive(Clone, Debug)]
pub(crate) enum CasePredicate {
    Compare {
        column: FuncArg,
        op: crate::model::CheckOp,
        literal: Value,
    },
    /// ADR 0238 — `expr BETWEEN low AND high` (inclusive).
    Between {
        column: FuncArg,
        low: Value,
        high: Value,
    },
    /// ADR 0238 — negated BETWEEN.
    NotBetween {
        column: FuncArg,
        low: Value,
        high: Value,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct CaseArm {
    pub(crate) predicate: CasePredicate,
    pub(crate) result: FuncArg,
}

/// ADR 0232 + 0233 — scalar function call in a SELECT
/// projection, now including CASE and extended string functions.
#[derive(Clone, Debug)]
pub(crate) enum FuncExpr {
    Upper(FuncArg),
    Lower(FuncArg),
    Length(FuncArg),
    Abs(FuncArg),
    Coalesce(Vec<FuncArg>),
    NullIf(FuncArg, FuncArg),
    /// ADR 0233 — variadic text concat (NULL propagates).
    Concat(Vec<FuncArg>),
    /// ADR 0233 — `SUBSTR(s, start[, length])`, 1-based `start`
    /// matching SQL convention; negative / zero `start` clamps
    /// to 1.
    Substr { s: FuncArg, start: FuncArg, length: Option<FuncArg> },
    /// ADR 0233 — remove ASCII whitespace from both ends.
    Trim(FuncArg),
    /// ADR 0233 — replace every occurrence of `from` with `to`.
    Replace { s: FuncArg, from: FuncArg, to: FuncArg },
    /// ADR 0233 — `CASE WHEN p THEN v [WHEN …] [ELSE v] END`.
    Case { arms: Vec<CaseArm>, else_result: Option<FuncArg> },
    /// ADR 0235 — `CAST(arg AS type)` conversion.
    Cast { arg: FuncArg, target: DataType },
    /// ADR 0239 — binary arithmetic. `op` is one of `+ - * / %`.
    BinOp { op: char, lhs: Box<FuncArg>, rhs: Box<FuncArg> },
    /// ADR 0236 — current wall-clock timestamp in microseconds
    /// since the Unix epoch, computed per-row (cheap).
    Now,
    /// ADR 0236 — add `seconds` to a `TimestampTz` argument.
    TimestampAdd { ts: FuncArg, seconds: FuncArg },
    /// ADR 0236 — `(a - b)` in whole seconds.
    TimestampDiff { a: FuncArg, b: FuncArg },
}

impl FuncExpr {
    fn label(&self) -> String {
        match self {
            FuncExpr::Upper(_) => "upper".into(),
            FuncExpr::Lower(_) => "lower".into(),
            FuncExpr::Length(_) => "length".into(),
            FuncExpr::Abs(_) => "abs".into(),
            FuncExpr::Coalesce(_) => "coalesce".into(),
            FuncExpr::NullIf(_, _) => "nullif".into(),
            FuncExpr::Concat(_) => "concat".into(),
            FuncExpr::Substr { .. } => "substr".into(),
            FuncExpr::Trim(_) => "trim".into(),
            FuncExpr::Replace { .. } => "replace".into(),
            FuncExpr::Case { .. } => "case".into(),
            FuncExpr::Cast { .. } => "cast".into(),
            FuncExpr::Now => "now".into(),
            FuncExpr::TimestampAdd { .. } => "timestamp_add".into(),
            FuncExpr::TimestampDiff { .. } => "timestamp_diff".into(),
            FuncExpr::BinOp { op, .. } => match op {
                '+' => "add".into(),
                '-' => "sub".into(),
                '*' => "mul".into(),
                '/' => "div".into(),
                '%' => "mod".into(),
                _ => "expr".into(),
            },
        }
    }
}

/// ADR 0217 + 0232 — one element of a SELECT list. A plain
/// column projection (ADR 0186+), a scalar aggregate, or a
/// scalar function call.
#[derive(Clone, Debug)]
pub(crate) enum SelectItem {
    Column(Projection),
    Agg(AggSpec),
    Func(FuncExpr),
}

impl SelectItem {
    pub(crate) fn is_aggregate(&self) -> bool { matches!(self, SelectItem::Agg(_)) }
    pub(crate) fn is_func(&self) -> bool { matches!(self, SelectItem::Func(_)) }
    pub(crate) fn label(&self) -> String {
        match self {
            SelectItem::Column(pj) => pj.label(),
            SelectItem::Agg(spec) => spec.result_column().to_string(),
            SelectItem::Func(fe) => fe.label(),
        }
    }
}

/// ADR 0233 + 0238 — coerce a CASE-predicate literal into its
/// `Value` form without a target column context.
fn coerce_case_literal(lit: Literal) -> SqlResult<Value> {
    Ok(match lit {
        Literal::Null => Value::Null,
        Literal::Bool(b) => Value::Boolean(b),
        Literal::Text(s) => Value::String(s),
        Literal::Number(s) => {
            if let Ok(n) = s.parse::<i64>() { Value::Int64(n) }
            else { return Err(SqlError::Type(format!(
                "CASE literal {s} is not a valid numeric"))); }
        }
        Literal::Default => return Err(SqlError::Syntax(
            "DEFAULT is not a CASE comparator".into())),
    })
}

/// ADR 0239 — coerce two numeric values into an `(i128, i128,
/// is_decimal)` triple for arithmetic. Integer operands widen
/// into `i128`; Decimal propagates the fixed-scale flag.  Non-
/// numeric operands reject with `SqlError::Type`.
fn numeric_pair(a: &Value, b: &Value) -> SqlResult<(i128, i128, bool)> {
    let (ai, ad) = numeric_as_i128(a)?;
    let (bi, bd) = numeric_as_i128(b)?;
    Ok((ai, bi, ad || bd))
}

fn numeric_as_i128(v: &Value) -> SqlResult<(i128, bool)> {
    match v {
        Value::Int32(n) => Ok((i128::from(*n), false)),
        Value::Int64(n) => Ok((i128::from(*n), false)),
        Value::Decimal(n) => Ok((*n, true)),
        other => Err(SqlError::Type(format!(
            "arithmetic requires numeric operands; got {other:?}"))),
    }
}

/// ADR 0235 — convert a `Value` to a target `DataType`.
/// NULL stays NULL. Unsupported pairs surface `SqlError::Type`.
pub(crate) fn cast_value(v: Value, target: &DataType) -> SqlResult<Value> {
    if matches!(v, Value::Null) { return Ok(Value::Null); }
    let invalid = || SqlError::Type(format!(
        "CAST: cannot convert {v:?} to {target:?}"));
    Ok(match (target, &v) {
        (DataType::Boolean, Value::Boolean(_)) => v,
        (DataType::Boolean, Value::Int32(n)) => Value::Boolean(*n != 0),
        (DataType::Boolean, Value::Int64(n)) => Value::Boolean(*n != 0),
        (DataType::Boolean, Value::String(s)) => match s.as_str() {
            "true" | "1" | "t" | "T" => Value::Boolean(true),
            "false" | "0" | "f" | "F" => Value::Boolean(false),
            _ => return Err(invalid()),
        },
        (DataType::Int32, Value::Int32(_)) => v,
        (DataType::Int32, Value::Int64(n)) =>
            Value::Int32((*n).try_into().map_err(|_| invalid())?),
        (DataType::Int32, Value::Decimal(n)) =>
            Value::Int32((*n).try_into().map_err(|_| invalid())?),
        (DataType::Int32, Value::String(s)) =>
            Value::Int32(s.parse().map_err(|_| invalid())?),
        (DataType::Int32, Value::Boolean(b)) =>
            Value::Int32(if *b { 1 } else { 0 }),
        (DataType::Int64, Value::Int64(_)) => v,
        (DataType::Int64, Value::Int32(n)) => Value::Int64(i64::from(*n)),
        (DataType::Int64, Value::Decimal(n)) =>
            Value::Int64((*n).try_into().map_err(|_| invalid())?),
        (DataType::Int64, Value::String(s)) =>
            Value::Int64(s.parse().map_err(|_| invalid())?),
        (DataType::Int64, Value::Boolean(b)) =>
            Value::Int64(if *b { 1 } else { 0 }),
        (DataType::Text | DataType::Varchar { .. }, Value::String(_)) => v,
        (DataType::Text | DataType::Varchar { .. }, Value::Int32(n)) =>
            Value::String(n.to_string()),
        (DataType::Text | DataType::Varchar { .. }, Value::Int64(n)) =>
            Value::String(n.to_string()),
        (DataType::Text | DataType::Varchar { .. }, Value::Decimal(n)) =>
            Value::String(n.to_string()),
        (DataType::Text | DataType::Varchar { .. }, Value::Boolean(b)) =>
            Value::String(if *b { "true".into() } else { "false".into() }),
        (DataType::Text | DataType::Varchar { .. }, Value::Json(s)) =>
            Value::String(s.clone()),
        (DataType::Json, Value::Json(_)) => v,
        (DataType::Json, Value::String(s)) =>
            if serde_json::from_str::<serde_json::Value>(s).is_ok() { Value::Json(s.clone()) }
            else { return Err(invalid()); },
        _ => return Err(invalid()),
    })
}

/// ADR 0235 — parse a type name after `AS` in a CAST.
fn parse_cast_target(p: &mut Parser) -> SqlResult<DataType> {
    let name = p.word()?;
    Ok(match name.to_ascii_lowercase().as_str() {
        "int" | "integer" | "int4" => DataType::Int32,
        "bigint" | "int8" => DataType::Int64,
        "text" => DataType::Text,
        "boolean" | "bool" => DataType::Boolean,
        "json" => DataType::Json,
        _ => return Err(SqlError::Unsupported(format!(
            "CAST target type {name} is not supported"))),
    })
}

/// ADR 0232 + 0234 — resolve a FuncArg against a row.
fn evaluate_func_arg(arg: &FuncArg, row: &[Value], table: &Table) -> SqlResult<Value> {
    match arg {
        FuncArg::Literal(v) => Ok(v.clone()),
        FuncArg::ColumnRef(name) => {
            let pos = locate(table, name)?;
            Ok(row[pos].clone())
        }
        FuncArg::Nested(fe) => evaluate_func(fe, row, table),
    }
}

/// ADR 0232 — evaluate a scalar function call against a row.
pub(crate) fn evaluate_func(fe: &FuncExpr, row: &[Value], table: &Table) -> SqlResult<Value> {
    match fe {
        FuncExpr::Upper(a) => match evaluate_func_arg(a, row, table)? {
            Value::Null => Ok(Value::Null),
            Value::String(s) => Ok(Value::String(s.to_uppercase())),
            other => Err(SqlError::Type(format!(
                "UPPER requires a text argument; got {other:?}"))),
        },
        FuncExpr::Lower(a) => match evaluate_func_arg(a, row, table)? {
            Value::Null => Ok(Value::Null),
            Value::String(s) => Ok(Value::String(s.to_lowercase())),
            other => Err(SqlError::Type(format!(
                "LOWER requires a text argument; got {other:?}"))),
        },
        FuncExpr::Length(a) => match evaluate_func_arg(a, row, table)? {
            Value::Null => Ok(Value::Null),
            Value::String(s) => Ok(Value::Int64(s.chars().count() as i64)),
            Value::Json(s) => Ok(Value::Int64(s.chars().count() as i64)),
            other => Err(SqlError::Type(format!(
                "LENGTH requires a text argument; got {other:?}"))),
        },
        FuncExpr::Abs(a) => match evaluate_func_arg(a, row, table)? {
            Value::Null => Ok(Value::Null),
            Value::Int32(n) => Ok(Value::Int32(n.abs())),
            Value::Int64(n) => Ok(Value::Int64(n.abs())),
            Value::Decimal(n) => Ok(Value::Decimal(n.abs())),
            other => Err(SqlError::Type(format!(
                "ABS requires a numeric argument; got {other:?}"))),
        },
        FuncExpr::Coalesce(args) => {
            for a in args {
                let v = evaluate_func_arg(a, row, table)?;
                if !matches!(v, Value::Null) { return Ok(v); }
            }
            Ok(Value::Null)
        }
        FuncExpr::NullIf(a, b) => {
            let va = evaluate_func_arg(a, row, table)?;
            let vb = evaluate_func_arg(b, row, table)?;
            if matches!(va, Value::Null) || matches!(vb, Value::Null) { return Ok(va); }
            if compare_values_for_check(&va, &vb) == std::cmp::Ordering::Equal {
                Ok(Value::Null)
            } else { Ok(va) }
        }
        FuncExpr::Concat(args) => {
            let mut out = String::new();
            for a in args {
                match evaluate_func_arg(a, row, table)? {
                    Value::Null => return Ok(Value::Null),
                    Value::String(s) | Value::Json(s) => out.push_str(&s),
                    Value::Int32(n) => out.push_str(&n.to_string()),
                    Value::Int64(n) => out.push_str(&n.to_string()),
                    Value::Decimal(n) => out.push_str(&n.to_string()),
                    Value::Boolean(b) => out.push_str(if b { "true" } else { "false" }),
                    other => return Err(SqlError::Type(format!(
                        "CONCAT cannot stringify {other:?}"))),
                }
            }
            Ok(Value::String(out))
        }
        FuncExpr::Substr { s, start, length } => {
            let sv = match evaluate_func_arg(s, row, table)? {
                Value::Null => return Ok(Value::Null),
                Value::String(s) => s,
                Value::Json(s) => s,
                other => return Err(SqlError::Type(format!(
                    "SUBSTR requires a text argument; got {other:?}"))),
            };
            let start_i = match evaluate_func_arg(start, row, table)? {
                Value::Null => return Ok(Value::Null),
                Value::Int32(n) => i64::from(n),
                Value::Int64(n) => n,
                other => return Err(SqlError::Type(format!(
                    "SUBSTR start must be an integer; got {other:?}"))),
            };
            let len_i = match length {
                None => None,
                Some(arg) => Some(match evaluate_func_arg(arg, row, table)? {
                    Value::Null => return Ok(Value::Null),
                    Value::Int32(n) => i64::from(n),
                    Value::Int64(n) => n,
                    other => return Err(SqlError::Type(format!(
                        "SUBSTR length must be an integer; got {other:?}"))),
                }),
            };
            let chars: Vec<char> = sv.chars().collect();
            // SQL-standard 1-based start; <= 0 clamps to 1.
            let start0 = if start_i < 1 { 0usize } else { (start_i - 1).min(chars.len() as i64) as usize };
            let end0 = match len_i {
                None => chars.len(),
                Some(l) if l <= 0 => start0,
                Some(l) => (start0 + l as usize).min(chars.len()),
            };
            Ok(Value::String(chars[start0..end0].iter().collect()))
        }
        FuncExpr::Trim(a) => match evaluate_func_arg(a, row, table)? {
            Value::Null => Ok(Value::Null),
            Value::String(s) => Ok(Value::String(s.trim().to_string())),
            Value::Json(s) => Ok(Value::String(s.trim().to_string())),
            other => Err(SqlError::Type(format!(
                "TRIM requires a text argument; got {other:?}"))),
        },
        FuncExpr::Replace { s, from, to } => {
            let sv = match evaluate_func_arg(s, row, table)? {
                Value::Null => return Ok(Value::Null),
                Value::String(s) => s,
                Value::Json(s) => s,
                other => return Err(SqlError::Type(format!(
                    "REPLACE requires text arguments; got {other:?}"))),
            };
            let fv = match evaluate_func_arg(from, row, table)? {
                Value::Null => return Ok(Value::Null),
                Value::String(s) | Value::Json(s) => s,
                other => return Err(SqlError::Type(format!(
                    "REPLACE 'from' must be text; got {other:?}"))),
            };
            let tv = match evaluate_func_arg(to, row, table)? {
                Value::Null => return Ok(Value::Null),
                Value::String(s) | Value::Json(s) => s,
                other => return Err(SqlError::Type(format!(
                    "REPLACE 'to' must be text; got {other:?}"))),
            };
            if fv.is_empty() {
                return Err(SqlError::Type(
                    "REPLACE 'from' must be non-empty".into()));
            }
            Ok(Value::String(sv.replace(&fv, &tv)))
        }
        FuncExpr::Cast { arg, target } => {
            let v = evaluate_func_arg(arg, row, table)?;
            cast_value(v, target)
        }
        FuncExpr::BinOp { op, lhs, rhs } => {
            let lv = evaluate_func_arg(lhs, row, table)?;
            let rv = evaluate_func_arg(rhs, row, table)?;
            if matches!(lv, Value::Null) || matches!(rv, Value::Null) { return Ok(Value::Null); }
            let (li, ri, is_decimal) = numeric_pair(&lv, &rv)?;
            let out = match op {
                '+' => li.checked_add(ri).ok_or_else(|| SqlError::Type("arithmetic overflow".into()))?,
                '-' => li.checked_sub(ri).ok_or_else(|| SqlError::Type("arithmetic overflow".into()))?,
                '*' => li.checked_mul(ri).ok_or_else(|| SqlError::Type("arithmetic overflow".into()))?,
                '/' => {
                    if ri == 0 { return Err(SqlError::Type("division by zero".into())); }
                    li.checked_div(ri).ok_or_else(|| SqlError::Type("arithmetic overflow".into()))?
                }
                '%' => {
                    if ri == 0 { return Err(SqlError::Type("modulo by zero".into())); }
                    li.checked_rem(ri).ok_or_else(|| SqlError::Type("arithmetic overflow".into()))?
                }
                _ => unreachable!("binop operator validated at parse"),
            };
            Ok(if is_decimal { Value::Decimal(out) }
                else { Value::Int64(out.try_into()
                    .map_err(|_| SqlError::Type("arithmetic result overflow".into()))?) })
        }
        FuncExpr::Now => {
            let now_us = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| SqlError::Type("NOW: system clock before epoch".into()))?
                .as_micros();
            let us: i64 = now_us.try_into()
                .map_err(|_| SqlError::Type("NOW: timestamp exceeds i64".into()))?;
            Ok(Value::TimestampTz(us))
        }
        FuncExpr::TimestampAdd { ts, seconds } => {
            let tv = evaluate_func_arg(ts, row, table)?;
            let sv = evaluate_func_arg(seconds, row, table)?;
            if matches!(tv, Value::Null) || matches!(sv, Value::Null) { return Ok(Value::Null); }
            let us = match tv {
                Value::TimestampTz(n) => n,
                other => return Err(SqlError::Type(format!(
                    "TIMESTAMP_ADD requires a TIMESTAMPTZ target; got {other:?}"))),
            };
            let delta_secs = match sv {
                Value::Int32(n) => i64::from(n),
                Value::Int64(n) => n,
                other => return Err(SqlError::Type(format!(
                    "TIMESTAMP_ADD seconds must be an integer; got {other:?}"))),
            };
            let delta_us = delta_secs.checked_mul(1_000_000)
                .ok_or_else(|| SqlError::Type("TIMESTAMP_ADD: seconds overflow".into()))?;
            let out = us.checked_add(delta_us)
                .ok_or_else(|| SqlError::Type("TIMESTAMP_ADD: timestamp overflow".into()))?;
            Ok(Value::TimestampTz(out))
        }
        FuncExpr::TimestampDiff { a, b } => {
            let av = evaluate_func_arg(a, row, table)?;
            let bv = evaluate_func_arg(b, row, table)?;
            if matches!(av, Value::Null) || matches!(bv, Value::Null) { return Ok(Value::Null); }
            let (ua, ub) = match (&av, &bv) {
                (Value::TimestampTz(x), Value::TimestampTz(y)) => (*x, *y),
                _ => return Err(SqlError::Type(
                    "TIMESTAMP_DIFF requires two TIMESTAMPTZ arguments".into())),
            };
            let diff_us = ua.checked_sub(ub)
                .ok_or_else(|| SqlError::Type("TIMESTAMP_DIFF: overflow".into()))?;
            Ok(Value::Int64(diff_us / 1_000_000))
        }
        FuncExpr::Case { arms, else_result } => {
            for arm in arms {
                let matched = match &arm.predicate {
                    CasePredicate::Compare { column, op, literal } => {
                        let v = evaluate_func_arg(column, row, table)?;
                        if matches!(v, Value::Null) { false }
                        else {
                            let ord = compare_values_for_check(&v, literal);
                            match op {
                                crate::model::CheckOp::Eq => ord == std::cmp::Ordering::Equal,
                                crate::model::CheckOp::Ne => ord != std::cmp::Ordering::Equal,
                                crate::model::CheckOp::Lt => ord == std::cmp::Ordering::Less,
                                crate::model::CheckOp::Le => ord != std::cmp::Ordering::Greater,
                                crate::model::CheckOp::Gt => ord == std::cmp::Ordering::Greater,
                                crate::model::CheckOp::Ge => ord != std::cmp::Ordering::Less,
                            }
                        }
                    }
                    CasePredicate::Between { column, low, high } => {
                        let v = evaluate_func_arg(column, row, table)?;
                        if matches!(v, Value::Null)
                            || matches!(low, Value::Null)
                            || matches!(high, Value::Null)
                        { false }
                        else {
                            compare_values_for_check(&v, low) != std::cmp::Ordering::Less
                                && compare_values_for_check(&v, high) != std::cmp::Ordering::Greater
                        }
                    }
                    CasePredicate::NotBetween { column, low, high } => {
                        let v = evaluate_func_arg(column, row, table)?;
                        if matches!(v, Value::Null)
                            || matches!(low, Value::Null)
                            || matches!(high, Value::Null)
                        { false }
                        else {
                            compare_values_for_check(&v, low) == std::cmp::Ordering::Less
                                || compare_values_for_check(&v, high) == std::cmp::Ordering::Greater
                        }
                    }
                };
                if matched { return evaluate_func_arg(&arm.result, row, table); }
            }
            match else_result {
                Some(e) => evaluate_func_arg(e, row, table),
                None => Ok(Value::Null),
            }
        }
    }
}

/// ADR 0232 — parse a FuncArg (column ref or literal) inside
/// function argument lists.
fn parse_func_primary(p: &mut Parser) -> SqlResult<FuncArg> {
    // ADR 0234 — try a nested function first; `consume_func`
    // returns None without advancing when the next word is
    // not a function head.
    if let Some(fe) = consume_func(p)? {
        return Ok(FuncArg::Nested(Box::new(fe)));
    }
    if p.next_is_literal() {
        let lit = p.literal()?;
        return Ok(FuncArg::Literal(match lit {
            Literal::Null => Value::Null,
            Literal::Bool(b) => Value::Boolean(b),
            Literal::Text(s) => Value::String(s),
            Literal::Number(s) => {
                if let Ok(n) = s.parse::<i64>() { Value::Int64(n) }
                else if let Ok(d) = s.parse::<i128>() { Value::Decimal(d) }
                else { return Err(SqlError::Type(format!(
                    "function literal {s} is not a valid numeric"))); }
            }
            Literal::Default => return Err(SqlError::Syntax(
                "DEFAULT is not a function argument".into())),
        }));
    }
    let first = p.word()?;
    if p.consume_symbol('.') {
        let second = p.word()?;
        Ok(FuncArg::ColumnRef(format!("{first}.{second}")))
    } else { Ok(FuncArg::ColumnRef(first)) }
}

/// ADR 0239 — multiplicative precedence tier (`*`, `/`, `%`).
fn parse_func_multiplicative(p: &mut Parser) -> SqlResult<FuncArg> {
    let mut lhs = parse_func_primary(p)?;
    loop {
        let op = if p.consume_symbol('*') { '*' }
            else if p.consume_symbol('/') { '/' }
            else if p.consume_symbol('%') { '%' }
            else { break };
        let rhs = parse_func_primary(p)?;
        lhs = FuncArg::Nested(Box::new(FuncExpr::BinOp {
            op, lhs: Box::new(lhs), rhs: Box::new(rhs),
        }));
    }
    Ok(lhs)
}

/// ADR 0239 — additive precedence tier (`+`, `-`), left-
/// associative over the multiplicative tier.
fn parse_func_arg(p: &mut Parser) -> SqlResult<FuncArg> {
    let mut lhs = parse_func_multiplicative(p)?;
    loop {
        let op = if p.consume_symbol('+') { '+' }
            else if p.consume_symbol('-') { '-' }
            else { break };
        let rhs = parse_func_multiplicative(p)?;
        lhs = FuncArg::Nested(Box::new(FuncExpr::BinOp {
            op, lhs: Box::new(lhs), rhs: Box::new(rhs),
        }));
    }
    Ok(lhs)
}

/// ADR 0232 — recognize a scalar function head. Returns `Some`
/// after consuming the tokens. The caller must ensure the next
/// word is one of the function names before calling.
fn consume_func(p: &mut Parser) -> SqlResult<Option<FuncExpr>> {
    if p.consume_word("upper") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Upper(a)))
    } else if p.consume_word("lower") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Lower(a)))
    } else if p.consume_word("length") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Length(a)))
    } else if p.consume_word("abs") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Abs(a)))
    } else if p.consume_word("coalesce") {
        p.symbol('(')?;
        let mut args = vec![parse_func_arg(p)?];
        while p.consume_symbol(',') { args.push(parse_func_arg(p)?); }
        p.symbol(')')?;
        if args.is_empty() {
            return Err(SqlError::Syntax(
                "COALESCE requires at least one argument".into()));
        }
        Ok(Some(FuncExpr::Coalesce(args)))
    } else if p.consume_word("nullif") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(',')?;
        let b = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::NullIf(a, b)))
    } else if p.consume_word("concat") {
        p.symbol('(')?;
        let mut args = vec![parse_func_arg(p)?];
        while p.consume_symbol(',') { args.push(parse_func_arg(p)?); }
        p.symbol(')')?;
        Ok(Some(FuncExpr::Concat(args)))
    } else if p.consume_word("substr") {
        p.symbol('(')?;
        let s = parse_func_arg(p)?;
        p.symbol(',')?;
        let start = parse_func_arg(p)?;
        let length = if p.consume_symbol(',') { Some(parse_func_arg(p)?) } else { None };
        p.symbol(')')?;
        Ok(Some(FuncExpr::Substr { s, start, length }))
    } else if p.consume_word("trim") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Trim(a)))
    } else if p.consume_word("replace") {
        p.symbol('(')?;
        let s = parse_func_arg(p)?;
        p.symbol(',')?;
        let from = parse_func_arg(p)?;
        p.symbol(',')?;
        let to = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Replace { s, from, to }))
    } else if p.consume_word("now") {
        p.symbol('(')?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Now))
    } else if p.consume_word("timestamp_add") {
        p.symbol('(')?;
        let ts = parse_func_arg(p)?;
        p.symbol(',')?;
        let seconds = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::TimestampAdd { ts, seconds }))
    } else if p.consume_word("timestamp_diff") {
        p.symbol('(')?;
        let a = parse_func_arg(p)?;
        p.symbol(',')?;
        let b = parse_func_arg(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::TimestampDiff { a, b }))
    } else if p.consume_word("cast") {
        p.symbol('(')?;
        let arg = parse_func_arg(p)?;
        p.expect("as")?;
        let target = parse_cast_target(p)?;
        p.symbol(')')?;
        Ok(Some(FuncExpr::Cast { arg, target }))
    } else if p.consume_word("case") {
        // ADR 0233 + 0238 — WHEN clause admits `col op lit` or
        // `col [NOT] BETWEEN low AND high`.
        let mut arms = Vec::new();
        while p.consume_word("when") {
            let column = parse_func_arg(p)?;
            let predicate = if p.consume_word("between") {
                let low = coerce_case_literal(p.literal()?)?;
                if !p.consume_word("and") {
                    return Err(SqlError::Syntax(
                        "CASE WHEN … BETWEEN low AND high requires AND".into()));
                }
                let high = coerce_case_literal(p.literal()?)?;
                CasePredicate::Between { column, low, high }
            } else if p.consume_word("not") {
                if !p.consume_word("between") {
                    return Err(SqlError::Unsupported(
                        "CASE WHEN NOT must be followed by BETWEEN".into()));
                }
                let low = coerce_case_literal(p.literal()?)?;
                if !p.consume_word("and") {
                    return Err(SqlError::Syntax(
                        "CASE WHEN … NOT BETWEEN low AND high requires AND".into()));
                }
                let high = coerce_case_literal(p.literal()?)?;
                CasePredicate::NotBetween { column, low, high }
            } else {
                let op = p.word_or_symbol_pair_for_check()?;
                let literal = coerce_case_literal(p.literal()?)?;
                CasePredicate::Compare { column, op, literal }
            };
            p.expect("then")?;
            let result = parse_func_arg(p)?;
            arms.push(CaseArm { predicate, result });
        }
        if arms.is_empty() {
            return Err(SqlError::Syntax("CASE requires at least one WHEN".into()));
        }
        let else_result = if p.consume_word("else") {
            Some(parse_func_arg(p)?)
        } else { None };
        p.expect("end")?;
        Ok(Some(FuncExpr::Case { arms, else_result }))
    } else {
        Ok(None)
    }
}

/// Parse one SELECT item: either an aggregate head or an
/// ordinary projection. Aggregates may appear anywhere in the
/// list; GROUP BY enforcement runs at planning time.
pub(crate) fn parse_select_item(p: &mut Parser) -> SqlResult<SelectItem> {
    if let Some(spec) = consume_aggregate(p)? { return Ok(SelectItem::Agg(spec)); }
    if let Some(fe) = consume_func(p)? {
        let full = parse_arith_tail(p, FuncArg::Nested(Box::new(fe)))?;
        return Ok(func_arg_as_select_item(full));
    }
    // ADR 0239 — bare literal projection: `SELECT 1 + 2 FROM t`.
    if p.next_is_literal() {
        let primary = parse_func_primary(p)?;
        let full = parse_arith_tail(p, primary)?;
        return Ok(func_arg_as_select_item(full));
    }
    let projection = parse_projection(p)?;
    if peek_is_arith_op(p) {
        let base = match &projection {
            Projection::Column(name) => FuncArg::ColumnRef(name.clone()),
            Projection::JsonPath { .. } => return Err(SqlError::Unsupported(
                "arithmetic on JSON-path projections is a future extension".into())),
        };
        let full = parse_arith_tail(p, base)?;
        return Ok(func_arg_as_select_item(full));
    }
    Ok(SelectItem::Column(projection))
}

fn peek_is_arith_op(p: &Parser) -> bool {
    matches!(p.peek(), Some(Token::Symbol('+')) | Some(Token::Symbol('-'))
        | Some(Token::Symbol('*')) | Some(Token::Symbol('/'))
        | Some(Token::Symbol('%')))
}

/// ADR 0239 — given a parsed left-hand primary, consume the
/// rest of the arithmetic expression at either precedence tier.
fn parse_arith_tail(p: &mut Parser, lhs: FuncArg) -> SqlResult<FuncArg> {
    let mut lhs = {
        let mut cur = lhs;
        loop {
            let op = if p.consume_symbol('*') { '*' }
                else if p.consume_symbol('/') { '/' }
                else if p.consume_symbol('%') { '%' }
                else { break };
            let rhs = parse_func_primary(p)?;
            cur = FuncArg::Nested(Box::new(FuncExpr::BinOp {
                op, lhs: Box::new(cur), rhs: Box::new(rhs),
            }));
        }
        cur
    };
    loop {
        let op = if p.consume_symbol('+') { '+' }
            else if p.consume_symbol('-') { '-' }
            else { break };
        let rhs = parse_func_multiplicative(p)?;
        lhs = FuncArg::Nested(Box::new(FuncExpr::BinOp {
            op, lhs: Box::new(lhs), rhs: Box::new(rhs),
        }));
    }
    Ok(lhs)
}

fn func_arg_as_select_item(arg: FuncArg) -> SelectItem {
    match arg {
        FuncArg::ColumnRef(name) => SelectItem::Column(Projection::Column(name)),
        FuncArg::Literal(v) => SelectItem::Func(FuncExpr::Coalesce(vec![FuncArg::Literal(v)])),
        FuncArg::Nested(fe) => SelectItem::Func(*fe),
    }
}

/// ADR 0218 — HAVING clause: a single comparison over a group
/// key or a scalar aggregate. No AND / OR for the MVP.
#[derive(Clone, Debug)]
pub(crate) struct HavingClause {
    pub(crate) term: HavingTerm,
    pub(crate) op: crate::model::CheckOp,
    pub(crate) literal: Literal,
}

#[derive(Clone, Debug)]
pub(crate) enum HavingTerm {
    GroupKey(String),
    Aggregate(AggSpec),
}

/// ADR 0218 — parse `HAVING term op literal` after GROUP BY.
/// Returns `None` when HAVING is absent. `term` is either an
/// aggregate call (COUNT(*), SUM(col), …) or a bare column
/// that must appear in `group_keys`.
pub(crate) fn parse_having(
    p: &mut Parser, table: &Table, group_keys: &[String],
) -> SqlResult<Option<HavingClause>> {
    if !p.consume_word("having") { return Ok(None); }
    if group_keys.is_empty() {
        return Err(SqlError::Unsupported(
            "HAVING requires a GROUP BY clause".into()));
    }
    let term = if let Some(spec) = consume_aggregate(p)? {
        if let AggSpec::Min(c) | AggSpec::Max(c) | AggSpec::Sum(c) | AggSpec::Avg(c)
            | AggSpec::CountCol(c) | AggSpec::CountDistinct(c) = &spec
        {
            locate(table, c)?;
        }
        // Expr-form aggregates resolve their columns per-row
        // via evaluate_func; no catalog check here.
        HavingTerm::Aggregate(spec)
    } else {
        // ADR 0228 — allow `alias.col` in HAVING key references.
        let name = parse_agg_column(p)?;
        if !group_keys.iter().any(|k| k == &name) {
            return Err(SqlError::Unsupported(
                "HAVING column must appear in GROUP BY or be an aggregate".into()));
        }
        locate(table, &name)?;
        HavingTerm::GroupKey(name)
    };
    let op = p.word_or_symbol_pair_for_check()?;
    let literal = p.literal()?;
    Ok(Some(HavingClause { term, op, literal }))
}

/// ADR 0217 — parse `GROUP BY col [, col]...` after the WHERE
/// clause. Returns an empty vec when GROUP BY is absent. Each
/// group key must resolve to a plain table column (no
/// expressions, no JSON paths, no aliases). Bounded to 16
/// keys to match `select_window`'s sort-key budget.
pub(crate) fn parse_group_by(p: &mut Parser, table: &Table) -> SqlResult<Vec<String>> {
    if !p.consume_word("group") { return Ok(Vec::new()); }
    p.expect("by")?;
    let mut keys = Vec::new();
    loop {
        // ADR 0228 — allow a two-part `alias.col` key for
        // joined queries; non-joined calls pass a single-table
        // schema so dotted names fail `locate` naturally.
        let name = parse_agg_column(p)?;
        locate(table, &name)?;
        if keys.len() == 16 {
            return Err(SqlError::Unsupported("GROUP BY exceeds 16 columns".into()));
        }
        if keys.iter().any(|k: &String| k == &name) {
            return Err(SqlError::Syntax("duplicate GROUP BY column".into()));
        }
        keys.push(name);
        if !p.consume_symbol(',') { break; }
    }
    Ok(keys)
}

/// ADR 0217 — evaluate a grouped SELECT. Rows are bucketed by
/// the composite group-key tuple using `compare_values_for_check`
/// so cross-type equivalents collapse to the same bucket (same
/// rule as `COUNT(DISTINCT col)` from ADR 0216). Buckets are
/// emitted in ascending group-key order. Bounded to 10 000
/// output groups to match the scan admission cap.
pub(crate) fn evaluate_grouped(
    table: &Table,
    rows: Vec<Vec<Value>>,
    group_keys: &[String],
    items: &[SelectItem],
    having: Option<&HavingClause>,
) -> SqlResult<SqlResultSet> {
    const MAX_GROUPS: usize = 10_000;
    let key_positions: Vec<usize> = group_keys.iter()
        .map(|name| locate(table, name)).collect::<SqlResult<Vec<_>>>()?;
    // (key, Vec<row>) buckets.
    let mut buckets: Vec<(Vec<Value>, Vec<Vec<Value>>)> = Vec::new();
    for row in rows {
        let key: Vec<Value> = key_positions.iter().map(|p| row[*p].clone()).collect();
        let position = buckets.iter().position(|(bk, _)|
            bk.iter().zip(&key).all(|(a, b)|
                compare_values_for_check(a, b) == std::cmp::Ordering::Equal));
        match position {
            Some(i) => buckets[i].1.push(row),
            None => {
                if buckets.len() == MAX_GROUPS {
                    return Err(SqlError::Unsupported(
                        "GROUP BY produced more than 10 000 distinct groups".into()));
                }
                buckets.push((key, vec![row]));
            }
        }
    }
    // Deterministic emission order: sort by key tuple using
    // the same comparator the scalar WHERE / ORDER BY path
    // uses. NULL sorts last.
    buckets.sort_by(|a, b| {
        for (x, y) in a.0.iter().zip(&b.0) {
            match (x, y) {
                (Value::Null, Value::Null) => continue,
                (Value::Null, _) => return std::cmp::Ordering::Greater,
                (_, Value::Null) => return std::cmp::Ordering::Less,
                (lhs, rhs) => {
                    let ord = compare_values_for_check(lhs, rhs);
                    if ord != std::cmp::Ordering::Equal { return ord; }
                }
            }
        }
        std::cmp::Ordering::Equal
    });
    let columns: Vec<String> = items.iter().map(|item| item.label()).collect();
    let mut output_rows: Vec<Vec<Value>> = Vec::with_capacity(buckets.len());
    for (key, bucket_rows) in &buckets {
        // ADR 0218 — HAVING filter runs per bucket before the
        // output row is materialized.
        if let Some(h) = having {
            let left = match &h.term {
                HavingTerm::GroupKey(name) => {
                    let idx = group_keys.iter().position(|k| k == name)
                        .expect("HAVING key validated");
                    key[idx].clone()
                }
                HavingTerm::Aggregate(spec) =>
                    evaluate_aggregate(spec, table, bucket_rows)?,
            };
            // Coerce the literal against the bucket value's
            // declared representation: numeric literals pass
            // through as-is for cross-type `compare_values_for_check`.
            let right = match (&left, &h.literal) {
                (_, Literal::Null) => Value::Null,
                (_, Literal::Text(s)) => Value::String(s.clone()),
                (_, Literal::Number(s)) =>
                    if let Ok(n) = s.parse::<i64>() { Value::Int64(n) }
                    else { return Err(SqlError::Type(
                        "HAVING numeric literal is not a valid Int64".into())); },
                (_, Literal::Bool(b)) => Value::Boolean(*b),
                (_, Literal::Default) => return Err(SqlError::Syntax(
                    "DEFAULT is not a HAVING literal".into())),
            };
            if matches!(left, Value::Null) || matches!(right, Value::Null) {
                continue;
            }
            let ord = compare_values_for_check(&left, &right);
            let keep = match h.op {
                crate::model::CheckOp::Eq => ord == std::cmp::Ordering::Equal,
                crate::model::CheckOp::Ne => ord != std::cmp::Ordering::Equal,
                crate::model::CheckOp::Lt => ord == std::cmp::Ordering::Less,
                crate::model::CheckOp::Le => ord != std::cmp::Ordering::Greater,
                crate::model::CheckOp::Gt => ord == std::cmp::Ordering::Greater,
                crate::model::CheckOp::Ge => ord != std::cmp::Ordering::Less,
            };
            if !keep { continue; }
        }
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            match item {
                SelectItem::Column(Projection::Column(name)) => {
                    let idx = group_keys.iter().position(|k| k == name)
                        .expect("bare column validated as group key");
                    out.push(key[idx].clone());
                }
                SelectItem::Column(_) => return Err(SqlError::Unsupported(
                    "GROUP BY projection must be a bare column; JSON paths are deferred".into())),
                SelectItem::Agg(spec) => {
                    out.push(evaluate_aggregate(spec, table, bucket_rows)?);
                }
                SelectItem::Func(_) => return Err(SqlError::Unsupported(
                    "scalar functions in grouped SELECT are a future extension".into())),
            }
        }
        output_rows.push(out);
    }
    Ok(SqlResultSet::Rows { columns, rows: output_rows })
}

/// ADR 0228 — parse an aggregate-argument column name that may
/// optionally be qualified (`alias.col`).  Reuses the single-word
/// path otherwise.
fn parse_agg_column(p: &mut Parser) -> SqlResult<String> {
    let first = p.word()?;
    if p.consume_symbol('.') {
        let second = p.word()?;
        Ok(format!("{first}.{second}"))
    } else { Ok(first) }
}

/// Recognize `COUNT(*)`, `MIN(col)`, `MAX(col)`, or `SUM(col)`
/// at the head of a SELECT. Returns `Some(spec)` after
/// consuming the tokens; returns `None` without advancing the
/// parser if the next token is not an aggregate keyword.
pub(crate) fn consume_aggregate(p: &mut Parser) -> SqlResult<Option<AggSpec>> {
    // ADR 0239 — classify a parsed aggregate arg into either
    // the backward-compat column-name form or the expression
    // form. Called after `parse_func_arg` so arithmetic / nested
    // function args naturally fall into the Expr variant.
    let classify = |arg: FuncArg,
                    column: fn(String) -> AggSpec,
                    expr: fn(FuncExpr) -> AggSpec| -> SqlResult<AggSpec> {
        Ok(match arg {
            FuncArg::ColumnRef(name) => column(name),
            FuncArg::Nested(fe) => expr(*fe),
            FuncArg::Literal(_) => return Err(SqlError::Unsupported(
                "aggregate argument cannot be a bare literal".into())),
        })
    };
    let agg = if p.consume_word("count") {
        p.symbol('(')?;
        let spec = if p.consume_symbol('*') { AggSpec::CountStar }
            else if p.consume_word("distinct") {
                let arg = parse_func_arg(p)?;
                classify(arg, AggSpec::CountDistinct, AggSpec::CountDistinctExpr)?
            } else {
                let arg = parse_func_arg(p)?;
                classify(arg, AggSpec::CountCol, AggSpec::CountColExpr)?
            };
        p.symbol(')')?;
        spec
    } else if p.consume_word("min") {
        p.symbol('(')?;
        let arg = parse_func_arg(p)?;
        let spec = classify(arg, AggSpec::Min, AggSpec::MinExpr)?;
        p.symbol(')')?;
        spec
    } else if p.consume_word("max") {
        p.symbol('(')?;
        let arg = parse_func_arg(p)?;
        let spec = classify(arg, AggSpec::Max, AggSpec::MaxExpr)?;
        p.symbol(')')?;
        spec
    } else if p.consume_word("sum") {
        p.symbol('(')?;
        let arg = parse_func_arg(p)?;
        let spec = classify(arg, AggSpec::Sum, AggSpec::SumExpr)?;
        p.symbol(')')?;
        spec
    } else if p.consume_word("avg") {
        p.symbol('(')?;
        let arg = parse_func_arg(p)?;
        let spec = classify(arg, AggSpec::Avg, AggSpec::AvgExpr)?;
        p.symbol(')')?;
        spec
    } else {
        return Ok(None);
    };
    Ok(Some(agg))
}

/// ADR 0214 — evaluate a non-COUNT aggregate against filtered
/// rows. Returns a single-row result or NULL when every
/// non-missing value is NULL. Non-numeric SUM targets are
/// rejected with `SqlError::Type`.
pub(crate) fn evaluate_aggregate(
    spec: &AggSpec, table: &Table, rows: &[Vec<Value>],
) -> SqlResult<Value> {
    match spec {
        AggSpec::CountStar => Ok(Value::Int64(rows.len() as i64)),
        AggSpec::Min(column) => {
            let position = locate(table, column)?;
            let mut best: Option<Value> = None;
            for row in rows {
                let v = &row[position];
                if matches!(v, Value::Null) { continue; }
                best = Some(match best {
                    None => v.clone(),
                    Some(cur) =>
                        if compare_values_for_check(v, &cur) == std::cmp::Ordering::Less
                            { v.clone() } else { cur },
                });
            }
            Ok(best.unwrap_or(Value::Null))
        }
        AggSpec::Max(column) => {
            let position = locate(table, column)?;
            let mut best: Option<Value> = None;
            for row in rows {
                let v = &row[position];
                if matches!(v, Value::Null) { continue; }
                best = Some(match best {
                    None => v.clone(),
                    Some(cur) =>
                        if compare_values_for_check(v, &cur) == std::cmp::Ordering::Greater
                            { v.clone() } else { cur },
                });
            }
            Ok(best.unwrap_or(Value::Null))
        }
        AggSpec::Sum(column) => {
            let position = locate(table, column)?;
            let col = &table.columns[position];
            match col.data_type {
                DataType::Int32 | DataType::Int64 | DataType::Decimal { .. } => {}
                _ => return Err(SqlError::Type(format!(
                    "SUM requires a numeric column; {column} is not numeric"))),
            };
            let mut sum: Option<i128> = None;
            for row in rows {
                let add = match row[position] {
                    Value::Null => continue,
                    Value::Int32(n) => i128::from(n),
                    Value::Int64(n) => i128::from(n),
                    Value::Decimal(n) => n,
                    _ => return Err(SqlError::Type(
                        "SUM encountered a non-numeric value".into())),
                };
                sum = Some(match sum {
                    None => add,
                    Some(cur) => cur.checked_add(add)
                        .ok_or_else(|| SqlError::Type(
                            "SUM overflow".into()))?,
                });
            }
            Ok(match (sum, &col.data_type) {
                (None, _) => Value::Null,
                // ADR 0214 — SUM of integers widens to Int64;
                // decimals preserve the column's native scale.
                (Some(v), DataType::Int32) => Value::Int64(v.try_into().map_err(|_|
                    SqlError::Type("SUM Int32 overflow".into()))?),
                (Some(v), DataType::Int64) => Value::Int64(v.try_into().map_err(|_|
                    SqlError::Type("SUM Int64 overflow".into()))?),
                (Some(v), DataType::Decimal { .. }) => Value::Decimal(v),
                _ => unreachable!("type check above excludes other variants"),
            })
        }
        // ADR 0216 — non-null count of one column.
        AggSpec::CountCol(column) => {
            let position = locate(table, column)?;
            let n = rows.iter()
                .filter(|row| !matches!(row[position], Value::Null))
                .count();
            Ok(Value::Int64(n as i64))
        }
        // ADR 0216 — distinct non-null count. Uses
        // `compare_values_for_check` so cross-type equivalents
        // (e.g. Int32 vs Int64, Json vs legacy String-encoded
        // JSON) collapse into the same bucket, matching the
        // scalar comparison semantics everywhere else.
        AggSpec::CountDistinct(column) => {
            let position = locate(table, column)?;
            let mut seen: Vec<Value> = Vec::new();
            for row in rows {
                if matches!(row[position], Value::Null) { continue; }
                if !seen.iter().any(|existing|
                    compare_values_for_check(existing, &row[position])
                        == std::cmp::Ordering::Equal)
                {
                    seen.push(row[position].clone());
                }
            }
            Ok(Value::Int64(seen.len() as i64))
        }
        // ADR 0237 — aggregate over scalar expression. Evaluates
        // the expression per row and feeds the result through
        // the same MIN / MAX / SUM / AVG / COUNT accumulators.
        AggSpec::MinExpr(fe) => {
            let mut best: Option<Value> = None;
            for row in rows {
                let v = evaluate_func(fe, row, table)?;
                if matches!(v, Value::Null) { continue; }
                best = Some(match best {
                    None => v,
                    Some(cur) =>
                        if compare_values_for_check(&v, &cur) == std::cmp::Ordering::Less
                            { v } else { cur },
                });
            }
            Ok(best.unwrap_or(Value::Null))
        }
        AggSpec::MaxExpr(fe) => {
            let mut best: Option<Value> = None;
            for row in rows {
                let v = evaluate_func(fe, row, table)?;
                if matches!(v, Value::Null) { continue; }
                best = Some(match best {
                    None => v,
                    Some(cur) =>
                        if compare_values_for_check(&v, &cur) == std::cmp::Ordering::Greater
                            { v } else { cur },
                });
            }
            Ok(best.unwrap_or(Value::Null))
        }
        AggSpec::SumExpr(fe) => {
            let mut sum: Option<i128> = None;
            let mut is_decimal = false;
            for row in rows {
                let add = match evaluate_func(fe, row, table)? {
                    Value::Null => continue,
                    Value::Int32(n) => i128::from(n),
                    Value::Int64(n) => i128::from(n),
                    Value::Decimal(n) => { is_decimal = true; n }
                    other => return Err(SqlError::Type(format!(
                        "SUM over expression encountered non-numeric {other:?}"))),
                };
                sum = Some(match sum {
                    None => add,
                    Some(cur) => cur.checked_add(add)
                        .ok_or_else(|| SqlError::Type("SUM overflow".into()))?,
                });
            }
            Ok(match sum {
                None => Value::Null,
                Some(v) if is_decimal => Value::Decimal(v),
                Some(v) => Value::Int64(v.try_into()
                    .map_err(|_| SqlError::Type("SUM result overflow".into()))?),
            })
        }
        AggSpec::AvgExpr(fe) => {
            let mut sum: i128 = 0;
            let mut count: i128 = 0;
            let mut is_decimal = false;
            for row in rows {
                let add = match evaluate_func(fe, row, table)? {
                    Value::Null => continue,
                    Value::Int32(n) => i128::from(n),
                    Value::Int64(n) => i128::from(n),
                    Value::Decimal(n) => { is_decimal = true; n }
                    other => return Err(SqlError::Type(format!(
                        "AVG over expression encountered non-numeric {other:?}"))),
                };
                sum = sum.checked_add(add)
                    .ok_or_else(|| SqlError::Type("AVG overflow".into()))?;
                count += 1;
            }
            if count == 0 { return Ok(Value::Null); }
            let divided = divide_round_half_even(sum, count)?;
            Ok(if is_decimal { Value::Decimal(divided) }
                else { Value::Int64(divided.try_into()
                    .map_err(|_| SqlError::Type("AVG result overflow".into()))?) })
        }
        AggSpec::CountColExpr(fe) => {
            let mut n = 0i64;
            for row in rows {
                if !matches!(evaluate_func(fe, row, table)?, Value::Null) { n += 1; }
            }
            Ok(Value::Int64(n))
        }
        AggSpec::CountDistinctExpr(fe) => {
            let mut seen: Vec<Value> = Vec::new();
            for row in rows {
                let v = evaluate_func(fe, row, table)?;
                if matches!(v, Value::Null) { continue; }
                if !seen.iter().any(|existing|
                    compare_values_for_check(existing, &v) == std::cmp::Ordering::Equal)
                { seen.push(v); }
            }
            Ok(Value::Int64(seen.len() as i64))
        }
        // ADR 0215 — arithmetic mean over non-null values.
        AggSpec::Avg(column) => {
            let position = locate(table, column)?;
            let col = &table.columns[position];
            match col.data_type {
                DataType::Int32 | DataType::Int64 | DataType::Decimal { .. } => {}
                _ => return Err(SqlError::Type(format!(
                    "AVG requires a numeric column; {column} is not numeric"))),
            };
            let mut sum: i128 = 0;
            let mut count: i128 = 0;
            for row in rows {
                let add = match row[position] {
                    Value::Null => continue,
                    Value::Int32(n) => i128::from(n),
                    Value::Int64(n) => i128::from(n),
                    Value::Decimal(n) => n,
                    _ => return Err(SqlError::Type(
                        "AVG encountered a non-numeric value".into())),
                };
                sum = sum.checked_add(add)
                    .ok_or_else(|| SqlError::Type("AVG overflow".into()))?;
                count += 1;
            }
            if count == 0 { return Ok(Value::Null); }
            // Half-even rounding keeps averages stable across
            // repeated accumulation and matches most SQL engines.
            let divided = divide_round_half_even(sum, count)?;
            Ok(match col.data_type {
                DataType::Int32 | DataType::Int64 => Value::Int64(divided.try_into()
                    .map_err(|_| SqlError::Type("AVG result overflow".into()))?),
                DataType::Decimal { .. } => Value::Decimal(divided),
                _ => unreachable!("type check above excludes other variants"),
            })
        }
    }
}

/// ADR 0215 — divide two i128 with half-even rounding of the
/// final integer result. Rejects divide-by-zero (callers must
/// skip when `count == 0`).
fn divide_round_half_even(numerator: i128, denominator: i128) -> SqlResult<i128> {
    if denominator == 0 {
        return Err(SqlError::Type("AVG divide by zero".into()));
    }
    let quotient = numerator / denominator;
    let remainder = numerator - quotient * denominator;
    let double_rem = remainder.checked_mul(2)
        .ok_or_else(|| SqlError::Type("AVG rounding overflow".into()))?;
    let abs_double_rem = double_rem.unsigned_abs();
    let abs_denom = denominator.unsigned_abs();
    let sign_adjust = if numerator.signum() * denominator.signum() >= 0 { 1 } else { -1 };
    Ok(match abs_double_rem.cmp(&abs_denom) {
        std::cmp::Ordering::Less => quotient,
        std::cmp::Ordering::Greater => quotient + sign_adjust,
        std::cmp::Ordering::Equal => {
            // Half: round to even.
            if quotient.rem_euclid(2) == 0 { quotient } else { quotient + sign_adjust }
        }
    })
}

/// ADR 0186 + ADR 0189 — parse a projection: identifier,
/// optionally followed by one or more `-> 'key'` / `->> 'key'`
/// segments. Intermediate segments must be `->`; a `->>`
/// segment can only appear at the end (SQL-standard: text
/// extraction terminates the path).
pub(crate) fn parse_projection(p: &mut Parser) -> SqlResult<Projection> {
    let first = p.word()?;
    // ADR 0226 — accept a two-part `table.column` reference for
    // joined SELECT. The lexer never admits `.` inside a word
    // so the check is unambiguous; non-join flows reject
    // dotted projections at resolution time.
    let column = if p.consume_symbol('.') {
        let second = p.word()?;
        format!("{first}.{second}")
    } else { first };
    parse_projection_tail(p, column)
}

/// ADR 0226 — split a projection column reference into
/// `(table_prefix, column_name)`. Returns `(None, name)` for
/// bare references.
pub(crate) fn split_qualified_column(s: &str) -> (Option<&str>, &str) {
    if let Some(dot) = s.find('.') { (Some(&s[..dot]), &s[dot + 1..]) }
    else { (None, s) }
}

/// ADR 0228 — build a synthetic `Table` describing the
/// composite row shape of a two-table join. Column names are
/// `alias.col`.  Only the fields needed by
/// `evaluate_grouped` / `evaluate_aggregate` / `locate` /
/// `compare_values_for_check` are populated; the rest take
/// their Default::default() values.
pub(crate) fn build_composite_table(
    left: &Table, left_alias: &str,
    right: &Table, right_alias: &str,
) -> Table {
    let mut columns: Vec<Column> = Vec::with_capacity(left.columns.len() + right.columns.len());
    for c in &left.columns {
        columns.push(Column {
            name: format!("{left_alias}.{}", c.name),
            data_type: c.data_type.clone(),
            nullable: true,
        });
    }
    for c in &right.columns {
        columns.push(Column {
            name: format!("{right_alias}.{}", c.name),
            data_type: c.data_type.clone(),
            nullable: true,
        });
    }
    Table {
        id: left.id,
        schema: left.schema,
        name: format!("{left_alias}+{right_alias}"),
        columns,
        primary_key: crate::model::PrimaryKey { columns: Vec::new() },
        unique_keys: Vec::new(),
        foreign_keys: Vec::new(),
        column_defaults: Default::default(),
        column_collations: Default::default(),
        policies: Default::default(),
        column_checks: Default::default(),
    }
}

/// ADR 0227 — INNER / OUTER / FULL join variant.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum JoinKind { Inner, LeftOuter, RightOuter, FullOuter }

/// ADR 0226 + 0227 — resolve an `ON a.c = b.c` reference pair
/// into `(left_column_position, right_column_position)`.  Each
/// side of ON must be a two-part `[alias_or_table, column]`
/// and must name the matching source table/alias.
pub(crate) fn join_resolve_on(
    left: &Table, left_alias: &str,
    right: &Table, right_alias: &str,
    left_ref: &[String], right_ref: &[String],
) -> SqlResult<(usize, usize)> {
    if left_ref.len() != 2 || right_ref.len() != 2 {
        return Err(SqlError::Unsupported(
            "JOIN ON clause requires `alias.col = alias.col`".into()));
    }
    let (lref_a, lref_c) = (&left_ref[0], &left_ref[1]);
    let (rref_a, rref_c) = (&right_ref[0], &right_ref[1]);
    if lref_a == left_alias && rref_a == right_alias {
        Ok((locate(left, lref_c)?, locate(right, rref_c)?))
    } else if lref_a == right_alias && rref_a == left_alias {
        Ok((locate(left, rref_c)?, locate(right, lref_c)?))
    } else {
        Err(SqlError::Resolution(
            "JOIN ON references do not match the FROM/JOIN aliases".into()))
    }
}

/// ADR 0226 + 0227 — resolve a qualified column reference against
/// a two-table join namespace.  Returns `(is_right_side, position,
/// column_name)`.
pub(crate) fn join_resolve_column(
    left: &Table, left_alias: &str,
    right: &Table, right_alias: &str,
    reference: &[String],
) -> SqlResult<(bool, usize, String)> {
    if reference.len() != 2 {
        return Err(SqlError::Unsupported(
            "JOIN references must be qualified as `alias.col`".into()));
    }
    let (table, col) = (&reference[0], &reference[1]);
    if table == left_alias {
        Ok((false, locate(left, col)?, col.clone()))
    } else if table == right_alias {
        Ok((true, locate(right, col)?, col.clone()))
    } else {
        Err(SqlError::Resolution(format!(
            "JOIN references unknown alias {table}")))
    }
}

/// ADR 0190 — parse the arrow chain after an already-consumed
/// column identifier. Used by both `parse_projection` and the
/// WHERE-side JSON path predicate parser so the chain grammar
/// has exactly one implementation.
/// ADR 0251 — parse a capability keyword (`READ`, `WRITE`,
/// `DDL`, `ADMIN`). Any other word returns `SqlError::Syntax`.
pub(crate) fn parse_capability(p: &mut Parser) -> SqlResult<bradian_core::Capability> {
    let word = p.word()?;
    match word.as_str() {
        "read"  => Ok(bradian_core::Capability::Read),
        "write" => Ok(bradian_core::Capability::Write),
        "ddl"   => Ok(bradian_core::Capability::Ddl),
        "admin" => Ok(bradian_core::Capability::Admin),
        _ => Err(SqlError::Syntax(format!(
            "expected capability (READ, WRITE, DDL, ADMIN); got {word}"))),
    }
}

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
    /// ADR 0243 — NULL-aware inequality. `extracted IS DISTINCT
    /// FROM literal` returns true when either side is NULL and
    /// the other is not, OR both are non-NULL and unequal. Both
    /// NULL is NOT distinct, so returns false.
    IsDistinctFrom { literal: Value },
    /// ADR 0243 — NULL-aware equality. Inverse of IsDistinctFrom:
    /// both-NULL is true; one-NULL-one-non-NULL is false; both
    /// non-NULL compares as ordinary equality.
    IsNotDistinctFrom { literal: Value },
}

#[derive(Clone, Debug)]
pub(crate) struct JsonPredicate {
    pub(crate) projection: Projection,
    pub(crate) matcher: JsonMatcher,
}

/// ADR 0240 — boolean tree over JSON-path predicates. Replaces
/// the ADR 0205 DNF when arbitrary parenthesization or `NOT`
/// prefix is used.
#[derive(Clone, Debug)]
pub(crate) enum WhereTree {
    Predicate(JsonPredicate),
    And(Box<WhereTree>, Box<WhereTree>),
    Or(Box<WhereTree>, Box<WhereTree>),
    Not(Box<WhereTree>),
}

#[derive(Clone, Debug)]
pub(crate) enum SelectWhere {
    KeyPredicate(Vec<Value>),
    /// ADR 0190 + ADR 0204 + ADR 0205 + ADR 0240 — JSON-path
    /// WHERE as a boolean tree. The tree degenerates to the
    /// earlier DNF shape (OR-of-AND groups) for queries that
    /// do not use NOT or parens, so all the existing matcher
    /// composition and admission behavior is preserved.
    JsonPathPredicates(WhereTree),
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
    // ADR 0240 — look one token ahead: `NOT`, `(`, or a word
    // followed by a JSON-arrow / hash-arrow dispatches to the
    // tree parser. Anything else goes to the PK-equality path.
    if p.next_is_word("not") || matches!(p.peek(), Some(Token::Symbol('('))) {
        let tree = parse_where_expr(p, table)?;
        return Ok(SelectWhere::JsonPathPredicates(tree));
    }
    let column_name = p.word()?;
    // ADR 0244 — a bare column followed by `IS` dispatches to
    // the tree parser so `WHERE scalar_col IS [NOT] NULL` and
    // `IS [NOT] DISTINCT FROM lit` work on non-JSON columns.
    // ADR 0245 — a bare column followed by `<` / `>` (which
    // covers `<`, `<=`, `<>`, `>`, `>=`) or by `=` on a
    // non-PK column also dispatches to the tree parser.
    let col_lower = column_name.to_ascii_lowercase();
    let is_pk_column = table.primary_key.columns.iter().any(|c| c == &col_lower);
    let next_is_lt_or_gt = matches!(p.peek(),
        Some(Token::Symbol('<')) | Some(Token::Symbol('>')));
    let next_is_eq_on_non_pk =
        matches!(p.peek(), Some(Token::Symbol('='))) && !is_pk_column;
    // ADR 0246 — bare-column IN / NOT / LIKE / ILIKE / BETWEEN
    // also dispatch to the tree parser.
    let next_is_matcher_keyword = p.next_is_word("in")
        || p.next_is_word("not")
        || p.next_is_word("like")
        || p.next_is_word("ilike")
        || p.next_is_word("between");
    let is_json_path = matches!(p.peek(),
        Some(Token::Arrow) | Some(Token::ArrowText)
        | Some(Token::Hash) | Some(Token::HashText))
        || p.next_is_word("is")
        || next_is_lt_or_gt
        || next_is_eq_on_non_pk
        || next_is_matcher_keyword;
    if is_json_path {
        p.restore_pos(saved);
        let tree = parse_where_expr(p, table)?;
        Ok(SelectWhere::JsonPathPredicates(tree))
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

/// ADR 0240 — parse a JSON-path WHERE as a boolean tree.
/// Grammar:
///   expr    := or_expr
///   or_expr := and_expr ( "OR" and_expr )*
///   and_expr:= unary   ( "AND" unary   )*
///   unary   := "NOT" unary | primary
///   primary := "(" expr ")" | predicate
fn parse_where_expr(p: &mut Parser, table: &Table) -> SqlResult<WhereTree> {
    parse_or_expr(p, table)
}

fn parse_or_expr(p: &mut Parser, table: &Table) -> SqlResult<WhereTree> {
    let mut lhs = parse_and_expr(p, table)?;
    while p.consume_word("or") {
        let rhs = parse_and_expr(p, table)?;
        lhs = WhereTree::Or(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

fn parse_and_expr(p: &mut Parser, table: &Table) -> SqlResult<WhereTree> {
    let mut lhs = parse_where_unary(p, table)?;
    while p.consume_word("and") {
        let rhs = parse_where_unary(p, table)?;
        lhs = WhereTree::And(Box::new(lhs), Box::new(rhs));
    }
    Ok(lhs)
}

fn parse_where_unary(p: &mut Parser, table: &Table) -> SqlResult<WhereTree> {
    if p.consume_word("not") {
        let inner = parse_where_unary(p, table)?;
        return Ok(WhereTree::Not(Box::new(inner)));
    }
    parse_where_primary(p, table)
}

fn parse_where_primary(p: &mut Parser, table: &Table) -> SqlResult<WhereTree> {
    if p.consume_symbol('(') {
        let inner = parse_where_expr(p, table)?;
        p.symbol(')')?;
        return Ok(inner);
    }
    Ok(WhereTree::Predicate(parse_json_predicate(p, table)?))
}

fn parse_json_predicate(p: &mut Parser, table: &Table) -> SqlResult<JsonPredicate> {
    let column_name = p.word()?;
    let projection = parse_projection_tail(p, column_name)?;
    let col_idx = locate(table, projection.source_column())?;
    let is_bare = matches!(&projection, Projection::Column(_));
    let as_text = match &projection {
        // ADR 0244 — bare columns only reach this point via the
        // `is` dispatcher; other matchers reject them below.
        Projection::Column(_) => false,
        Projection::JsonPath { as_text, .. } => *as_text,
    };
    // ADR 0246 — bare-column RHS uses `coerce(lit, column)`
    // across every matcher; JSON-path RHS keeps as_text-aware
    // coercion. LIKE / ILIKE also require a text bare column.
    let column_ref = if is_bare { Some(&table.columns[col_idx]) } else { None };
    let check_text_column = |keyword: &'static str| -> SqlResult<()> {
        if let Some(col) = column_ref {
            match col.data_type {
                DataType::Text | DataType::Varchar { .. } => Ok(()),
                _ => Err(SqlError::Type(format!(
                    "{keyword} requires a text column; {} is {:?}",
                    col.name, col.data_type))),
            }
        } else { Ok(()) }
    };
    let matcher = if p.consume_word("in") {
        JsonMatcher::In { literals: parse_literal_list(p, column_ref, as_text)? }
    } else if p.consume_word("like") {
        check_text_column("LIKE")?;
        let pattern = parse_like_pattern(p)?;
        let escape = parse_optional_escape(p)?;
        JsonMatcher::Like { pattern, escape }
    } else if p.consume_word("ilike") {
        check_text_column("ILIKE")?;
        let pattern = parse_like_pattern(p)?.to_ascii_lowercase();
        let escape = parse_optional_escape(p)?.map(|c| c.to_ascii_lowercase());
        JsonMatcher::Ilike { pattern_lower: pattern, escape }
    } else if p.consume_word("between") {
        parse_between_tail(p, column_ref, as_text, /*negated=*/false)?
    } else if p.consume_word("is") {
        let negated = p.consume_word("not");
        if p.consume_word("null") {
            if negated { JsonMatcher::IsNotNull } else { JsonMatcher::IsNull }
        } else if p.consume_word("distinct") {
            p.expect("from")?;
            let literal = p.literal()?;
            let value = coerce_rhs_literal(&literal, column_ref, as_text)?;
            if negated { JsonMatcher::IsNotDistinctFrom { literal: value } }
            else { JsonMatcher::IsDistinctFrom { literal: value } }
        } else {
            return Err(SqlError::Unsupported(
                "IS supports [NOT] NULL and [NOT] DISTINCT FROM".into()));
        }
    } else if p.consume_word("not") {
        if p.consume_word("in") {
            JsonMatcher::NotIn { literals: parse_literal_list(p, column_ref, as_text)? }
        } else if p.consume_word("between") {
            parse_between_tail(p, column_ref, as_text, /*negated=*/true)?
        } else if p.consume_word("like") {
            check_text_column("NOT LIKE")?;
            let pattern = parse_like_pattern(p)?;
            let escape = parse_optional_escape(p)?;
            JsonMatcher::NotLike { pattern, escape }
        } else if p.consume_word("ilike") {
            check_text_column("NOT ILIKE")?;
            let pattern = parse_like_pattern(p)?.to_ascii_lowercase();
            let escape = parse_optional_escape(p)?.map(|c| c.to_ascii_lowercase());
            JsonMatcher::NotIlike { pattern_lower: pattern, escape }
        } else {
            return Err(SqlError::Unsupported(
                "NOT must be followed by IN, BETWEEN, LIKE, or ILIKE after a predicate (or placed as a boolean prefix)".into()));
        }
    } else {
        let op = p.word_or_symbol_pair_for_check()?;
        let literal = p.literal()?;
        let value = coerce_rhs_literal(&literal, column_ref, as_text)?;
        JsonMatcher::Compare { op, literal: value }
    };
    Ok(JsonPredicate { projection, matcher })
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

/// ADR 0246 — coerce a WHERE-RHS literal through the correct
/// path. Bare non-JSON columns coerce via the declared column
/// type (`coerce(lit, column)`); JSON paths keep the existing
/// as_text-aware JSON coercion.
fn coerce_rhs_literal(
    lit: &Literal, column: Option<&Column>, as_text: bool,
) -> SqlResult<Value> {
    match column {
        Some(col) => coerce(lit, col),
        None => coerce_where_literal(lit, as_text),
    }
}

/// ADR 0208 — parse `low AND high` as the RHS of a BETWEEN /
/// NOT BETWEEN predicate. The `AND` here is a grammar keyword,
/// not a DNF separator.  Inclusive on both ends. `low > high`
/// is rejected at parse (empty range would be surprising to
/// readers). ADR 0246 — accepts a bare-column handle so
/// low/high coerce via the column's declared type.
fn parse_between_tail(
    p: &mut Parser, column: Option<&Column>, as_text: bool, negated: bool,
) -> SqlResult<JsonMatcher> {
    let low_lit = p.literal()?;
    let low = coerce_rhs_literal(&low_lit, column, as_text)?;
    if !p.consume_word("and") {
        return Err(SqlError::Syntax(
            "BETWEEN requires `AND` between low and high".into()));
    }
    let high_lit = p.literal()?;
    let high = coerce_rhs_literal(&high_lit, column, as_text)?;
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
/// NOT IN predicate. ADR 0246 — accepts a bare-column handle
/// so each literal coerces via the column's declared type.
/// At least one value is required.
fn parse_literal_list(
    p: &mut Parser, column: Option<&Column>, as_text: bool,
) -> SqlResult<Vec<Value>> {
    p.symbol('(')?;
    let mut values = Vec::new();
    loop {
        let literal = p.literal()?;
        values.push(coerce_rhs_literal(&literal, column, as_text)?);
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
/// ADR 0240 — evaluate a single `JsonPredicate` against one
/// row, returning `Some(bool)` for the row's truth value.  NULL
/// three-valued logic is handled as follows: IS [NOT] NULL
/// returns `Some(...)` always; every other matcher returns
/// `Some(false)` on NULL-extracted (preserving ADR 0206's
/// semantics for AND / OR).  Callers wrap `Some(false)` in
/// `WhereTree::Not` to produce `Some(true)` under NOT.
fn eval_predicate(
    pred: &JsonPredicate, row: &[Value], table: &Table,
) -> SqlResult<bool> {
    let position = locate(table, pred.projection.source_column())?;
    let extracted = apply_projection(&pred.projection, row[position].clone())?;
    match &pred.matcher {
        JsonMatcher::IsNull => return Ok(matches!(extracted, Value::Null)),
        JsonMatcher::IsNotNull => return Ok(!matches!(extracted, Value::Null)),
        // ADR 0243 — IS [NOT] DISTINCT FROM is NULL-aware and
        // must see the extracted value even when it is NULL.
        JsonMatcher::IsDistinctFrom { literal } => {
            let ex_null = matches!(extracted, Value::Null);
            let lit_null = matches!(literal, Value::Null);
            return Ok(match (ex_null, lit_null) {
                (true, true) => false,
                (true, false) | (false, true) => true,
                (false, false) =>
                    compare_values_for_check(&extracted, literal) != std::cmp::Ordering::Equal,
            });
        }
        JsonMatcher::IsNotDistinctFrom { literal } => {
            let ex_null = matches!(extracted, Value::Null);
            let lit_null = matches!(literal, Value::Null);
            return Ok(match (ex_null, lit_null) {
                (true, true) => true,
                (true, false) | (false, true) => false,
                (false, false) =>
                    compare_values_for_check(&extracted, literal) == std::cmp::Ordering::Equal,
            });
        }
        _ => {}
    }
    if matches!(extracted, Value::Null) { return Ok(false); }
    Ok(match &pred.matcher {
        JsonMatcher::Compare { op, literal } => {
            if matches!(literal, Value::Null) { false }
            else {
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
        }
        JsonMatcher::In { literals } => literals.iter().any(|lit|
            !matches!(lit, Value::Null)
                && compare_values_for_check(&extracted, lit)
                    == std::cmp::Ordering::Equal),
        JsonMatcher::NotIn { literals } => literals.iter().all(|lit|
            !matches!(lit, Value::Null)
                && compare_values_for_check(&extracted, lit)
                    != std::cmp::Ordering::Equal),
        JsonMatcher::Between { low, high } =>
            !matches!(low, Value::Null) && !matches!(high, Value::Null)
                && compare_values_for_check(&extracted, low) != std::cmp::Ordering::Less
                && compare_values_for_check(&extracted, high) != std::cmp::Ordering::Greater,
        JsonMatcher::NotBetween { low, high } =>
            !matches!(low, Value::Null) && !matches!(high, Value::Null)
                && (compare_values_for_check(&extracted, low) == std::cmp::Ordering::Less
                    || compare_values_for_check(&extracted, high) == std::cmp::Ordering::Greater),
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
        JsonMatcher::IsNull | JsonMatcher::IsNotNull
        | JsonMatcher::IsDistinctFrom { .. } | JsonMatcher::IsNotDistinctFrom { .. } =>
            unreachable!("handled above the NULL guard"),
    })
}

/// ADR 0240 — walk a `WhereTree` and return the row's truth
/// value. Short-circuiting applies at AND / OR nodes.
fn eval_where_tree(
    tree: &WhereTree, row: &[Value], table: &Table,
) -> SqlResult<bool> {
    match tree {
        WhereTree::Predicate(pred) => eval_predicate(pred, row, table),
        WhereTree::And(a, b) => {
            if !eval_where_tree(a, row, table)? { return Ok(false); }
            eval_where_tree(b, row, table)
        }
        WhereTree::Or(a, b) => {
            if eval_where_tree(a, row, table)? { return Ok(true); }
            eval_where_tree(b, row, table)
        }
        WhereTree::Not(inner) => Ok(!eval_where_tree(inner, row, table)?),
    }
}

pub(crate) fn apply_where_filter(
    rows: Vec<Vec<Value>>,
    table: &Table,
    tree: &WhereTree,
) -> SqlResult<Vec<Vec<Value>>> {
    let mut kept = Vec::with_capacity(rows.len());
    for row in rows {
        if eval_where_tree(tree, &row, table)? {
            kept.push(row);
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
        // ADR 0219 — DISTINCT / GROUP BY / COUNT(DISTINCT)
        // treat two NULLs as equal but NULL and any concrete
        // value as distinct. Scalar WHERE / IN / BETWEEN
        // short-circuit NULL above this layer, so promoting
        // NULL to a dedicated branch does not disturb them.
        (Value::Null, Value::Null) => std::cmp::Ordering::Equal,
        (Value::Null, _) => std::cmp::Ordering::Less,
        (_, Value::Null) => std::cmp::Ordering::Greater,
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
    /// ADR 0249 — WAL-backed user catalog. Opened at
    /// `<root>/users`; separate journal from the DDL catalog.
    users: crate::persistent_user_catalog::PersistentUserCatalog,
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
        let users = crate::persistent_user_catalog::PersistentUserCatalog::open(
            root.join("users"),
        ).map_err(|e| SqlError::Table(crate::typed_table::TableError::Schema(
            format!("open user catalog: {e}"),
        )))?;
        Ok(Self {
            root: root.to_owned(),
            catalog,
            operation: Mutex::new(()),
            bace_wiring: wiring,
            global_budget: crate::workspace_usage::GlobalBudget::default(),
            users,
        })
    }

    /// ADR 0249 — verify a user's plaintext password against
    /// the persistent user catalog. Returns Ok(()) only for
    /// known, active users with matching passwords.
    pub fn verify_password(&self, user: &str, password: &str) -> SqlResult<()> {
        self.users.verify_password(user, password)?;
        Ok(())
    }

    /// ADR 0250 — verify credentials, derive a principal, and
    /// run `callback` against an `AuthenticatedSession`. Every
    /// `execute` on that session goes through `execute_as` so
    /// capability checks fire before the statement runs.
    ///
    /// ADR 0251 — capabilities come from the stored
    /// `UserRecord`. A user with no capabilities can
    /// authenticate but every SQL statement fast-fails with
    /// `SqlError::Authz`.
    pub fn with_authenticated_session<R>(
        &self, user: &str, password: &str,
        callback: impl FnOnce(&AuthenticatedSession<'_>) -> SqlResult<R>,
    ) -> SqlResult<R> {
        self.users.verify_password(user, password)?;
        let snapshot = self.users.snapshot();
        let record = snapshot.get(user)
            .ok_or_else(|| SqlError::Resolution(format!("user {user} not found after verify")))?;
        let caps: Vec<bradian_core::Capability> =
            record.capabilities().iter().copied().collect();
        let authz = bradian_core::AuthorizationContext::new(record.name.clone(), caps);
        let session = AuthenticatedSession { engine: self, authz };
        callback(&session)
    }

    /// ADR 0249 — snapshot the current user catalog. Useful
    /// for tests and diagnostics; does not reveal plaintext.
    pub fn user_catalog_snapshot(&self) -> crate::user_catalog::UserCatalog {
        self.users.snapshot()
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
        // ADR 0229 — resolve `IN (SELECT …)` subqueries by
        // executing each eagerly and rewriting the SQL with a
        // literal list.  This runs BEFORE the UNION splitter so
        // a subquery inside a UNION arm still gets expanded.
        let sql = self.resolve_in_subqueries(sql, control)?;
        let sql = sql.as_str();
        // ADR 0225 — UNION / UNION ALL at the top level. Each
        // arm runs through `execute_impl_single` with its own
        // short-lived mutex guard; the results are combined and
        // returned as a single `Rows` shape.
        if let Some(parts) = split_top_level_unions(sql) {
            if parts.len() >= 2 {
                let mut parts_iter = parts.into_iter();
                let (_, first_sql) = parts_iter.next().expect("at least one part");
                let first = self.execute_impl_single(
                    &first_sql, control, budget.as_deref_mut(), policy, authz)?;
                let (mut columns, mut rows) = match first {
                    SqlResultSet::Rows { columns, rows } => (columns, rows),
                    _ => return Err(SqlError::Unsupported(
                        "UNION requires every arm to be a SELECT".into())),
                };
                let mut any_dedup = false;
                for (all, piece) in parts_iter {
                    let next = self.execute_impl_single(
                        &piece, control, budget.as_deref_mut(), policy, authz)?;
                    let (next_cols, next_rows) = match next {
                        SqlResultSet::Rows { columns, rows } => (columns, rows),
                        _ => return Err(SqlError::Unsupported(
                            "UNION requires every arm to be a SELECT".into())),
                    };
                    if next_cols.len() != columns.len() {
                        return Err(SqlError::Unsupported(
                            "UNION arms must have the same column count".into()));
                    }
                    rows.extend(next_rows);
                    if !all { any_dedup = true; }
                    let _ = next_cols;
                }
                if any_dedup {
                    rows = dedup_rows(rows);
                    sort_distinct_output(&mut rows);
                }
                let _ = &mut columns;
                return Ok(SqlResultSet::Rows { columns, rows });
            }
        }
        self.execute_impl_single(sql, control, budget, policy, authz)
    }

    /// ADR 0226 — execute a simple INNER JOIN between two tables.
    /// Builds the composite row namespace, resolves qualified
    /// column references, runs a nested-loop join under the
    /// operation mutex, applies the optional WHERE, and emits
    /// projected rows.  No aggregates, DISTINCT, ORDER BY, or
    /// ADR 0225 UNION composition — each is a separate ADR.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    fn execute_inner_join(
        &self,
        kind: JoinKind,
        left: &Table, left_alias: &str,
        right: &Table, right_alias: &str,
        left_ref: &[String],
        right_ref: &[String],
        select_items: &[SelectItem],
        caller_wildcard: bool,
        where_clause: Option<(Vec<String>, Literal)>,
        group_keys: Vec<String>,
        having: Option<HavingClause>,
        composite: &Table,
        _control: &ScanControl,
        mut budget: Option<&mut crate::query_budget::BudgetTracker>,
    ) -> SqlResult<SqlResultSet> {
        let (left_pos, right_pos) =
            join_resolve_on(left, left_alias, right, right_alias, left_ref, right_ref)?;
        let left_store = self.table_store(left)?;
        let right_store = self.table_store(right)?;
        let left_rows = left_store.scan()?;
        let right_rows = right_store.scan()?;
        if let Some(b) = budget.as_mut() {
            let total_rows = (left_rows.len() + right_rows.len()) as u64;
            b.charge_rows(total_rows)?;
            b.charge_bytes(crate::query_budget::rows_bytes(&left_rows)
                + crate::query_budget::rows_bytes(&right_rows))?;
        }
        // ADR 0228 — aggregate / GROUP BY path: build composite
        // rows and dispatch to the single-table evaluator with
        // the synthetic composite schema.
        let any_aggregate = select_items.iter().any(|it| it.is_aggregate());
        if any_aggregate || !group_keys.is_empty() {
            return self.execute_join_grouped(
                kind, left, left_alias, right, right_alias,
                left_pos, right_pos, left_rows, right_rows,
                select_items, caller_wildcard,
                where_clause, group_keys, having, composite,
            );
        }
        let projections: Vec<(String, String)> = if caller_wildcard {
            left.columns.iter().map(|c| (left_alias.to_string(), c.name.clone()))
                .chain(right.columns.iter().map(|c| (right_alias.to_string(), c.name.clone())))
                .collect()
        } else {
            select_items.iter().map(|item| match item {
                SelectItem::Column(Projection::Column(s)) => {
                    let (prefix, col) = split_qualified_column(s);
                    let table = prefix.ok_or_else(|| SqlError::Unsupported(
                        "joined SELECT requires qualified column references".into()))?;
                    Ok((table.to_string(), col.to_string()))
                }
                _ => Err(SqlError::Unsupported(
                    "joined SELECT accepts bare qualified columns only (no aggregates, JSON paths, ORDER BY, …)".into())),
            }).collect::<SqlResult<Vec<_>>>()?
        };
        let proj_positions: Vec<(bool, usize)> = projections.iter().map(|(alias, col)| {
            if alias == left_alias { Ok((false, locate(left, col)?)) }
            else if alias == right_alias { Ok((true, locate(right, col)?)) }
            else { Err(SqlError::Resolution(format!(
                "joined SELECT references unknown alias {alias}"))) }
        }).collect::<SqlResult<Vec<_>>>()?;
        let columns: Vec<String> = projections.iter()
            .map(|(t, c)| format!("{t}.{c}")).collect();
        let where_filter: Option<(bool, usize, Value)> = match where_clause {
            None => None,
            Some((col_ref, lit)) => {
                let (side, pos, _) =
                    join_resolve_column(left, left_alias, right, right_alias, &col_ref)?;
                let column_def = if side { &right.columns[pos] } else { &left.columns[pos] };
                Some((side, pos, coerce(&lit, column_def)?))
            }
        };
        let left_nulls: Vec<Value> = vec![Value::Null; left.columns.len()];
        let right_nulls: Vec<Value> = vec![Value::Null; right.columns.len()];
        let mut rows: Vec<Vec<Value>> = Vec::new();
        let mut left_matched = vec![false; left_rows.len()];
        let mut right_matched = vec![false; right_rows.len()];
        let emit = |lrow: &[Value], rrow: &[Value], out: &mut Vec<Vec<Value>>|
            -> SqlResult<()> {
                if let Some((side, pos, ref lit)) = where_filter {
                    let v = if side { &rrow[pos] } else { &lrow[pos] };
                    if matches!(v, Value::Null)
                        || compare_values_for_check(v, lit)
                            != std::cmp::Ordering::Equal
                    { return Ok(()); }
                }
                let o: Vec<Value> = proj_positions.iter().map(|(is_right, pos)|
                    if *is_right { rrow[*pos].clone() } else { lrow[*pos].clone() }
                ).collect();
                out.push(o);
                Ok(())
            };
        for (li, lrow) in left_rows.iter().enumerate() {
            for (ri, rrow) in right_rows.iter().enumerate() {
                let l = &lrow[left_pos];
                let r = &rrow[right_pos];
                if matches!(l, Value::Null) || matches!(r, Value::Null) { continue; }
                if compare_values_for_check(l, r) != std::cmp::Ordering::Equal { continue; }
                left_matched[li] = true;
                right_matched[ri] = true;
                emit(lrow, rrow, &mut rows)?;
            }
        }
        // ADR 0227 — emit left-side orphans for LEFT/FULL.
        if matches!(kind, JoinKind::LeftOuter | JoinKind::FullOuter) {
            for (li, lrow) in left_rows.iter().enumerate() {
                if !left_matched[li] {
                    emit(lrow, &right_nulls, &mut rows)?;
                }
            }
        }
        // ADR 0227 — emit right-side orphans for RIGHT/FULL.
        if matches!(kind, JoinKind::RightOuter | JoinKind::FullOuter) {
            for (ri, rrow) in right_rows.iter().enumerate() {
                if !right_matched[ri] {
                    emit(&left_nulls, rrow, &mut rows)?;
                }
            }
        }
        Ok(SqlResultSet::Rows { columns, rows })
    }

    /// ADR 0228 — build composite rows from a two-table join
    /// and run them through `evaluate_grouped` /
    /// `evaluate_aggregate` using a synthetic composite table
    /// so the same grouping path serves single-table and
    /// joined queries.
    #[allow(clippy::too_many_arguments)]
    fn execute_join_grouped(
        &self,
        kind: JoinKind,
        left: &Table, _left_alias: &str,
        right: &Table, _right_alias: &str,
        left_pos: usize, right_pos: usize,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
        select_items: &[SelectItem],
        _caller_wildcard: bool,
        where_clause: Option<(Vec<String>, Literal)>,
        group_keys: Vec<String>,
        having: Option<HavingClause>,
        composite: &Table,
    ) -> SqlResult<SqlResultSet> {
        // ADR 0228 — translate select_items that referenced
        // `alias.col` into the composite's `alias.col` column
        // name (the Projection variant already stores it that
        // way; only the aggregate variants rebuild their
        // column reference verbatim).
        let items: Vec<SelectItem> = select_items.iter().map(|item| match item {
            SelectItem::Column(Projection::Column(s)) if !s.contains('.') =>
                Err(SqlError::Unsupported(
                    "joined SELECT requires qualified column references".into())),
            _ => Ok(item.clone()),
        }).collect::<SqlResult<Vec<_>>>()?;
        // Nested-loop into composite rows.
        let where_filter: Option<(bool, usize, Value)> = match where_clause {
            None => None,
            Some((col_ref, lit)) => {
                // Need original aliases here — recover from `composite`
                // column naming. The composite prefix is `{alias}.`.
                let (alias, col) = match col_ref.as_slice() {
                    [a, c] => (a.clone(), c.clone()),
                    _ => return Err(SqlError::Unsupported(
                        "joined WHERE requires `alias.col` reference".into())),
                };
                // Resolve against the raw tables to discover the side.
                let composite_name = format!("{alias}.{col}");
                let composite_pos = locate(composite, &composite_name)?;
                let column_def = &composite.columns[composite_pos];
                let value = coerce(&lit, column_def)?;
                let side = composite_pos >= left.columns.len();
                let pos_in_source = if side {
                    composite_pos - left.columns.len()
                } else { composite_pos };
                Some((side, pos_in_source, value))
            }
        };
        let left_nulls: Vec<Value> = vec![Value::Null; left.columns.len()];
        let right_nulls: Vec<Value> = vec![Value::Null; right.columns.len()];
        let mut composite_rows: Vec<Vec<Value>> = Vec::new();
        let mut left_matched = vec![false; left_rows.len()];
        let mut right_matched = vec![false; right_rows.len()];
        let emit = |lrow: &[Value], rrow: &[Value], out: &mut Vec<Vec<Value>>|
            -> SqlResult<()> {
                if let Some((side, pos, ref lit)) = where_filter {
                    let v = if side { &rrow[pos] } else { &lrow[pos] };
                    if matches!(v, Value::Null)
                        || compare_values_for_check(v, lit)
                            != std::cmp::Ordering::Equal
                    { return Ok(()); }
                }
                let mut composite = Vec::with_capacity(lrow.len() + rrow.len());
                composite.extend_from_slice(lrow);
                composite.extend_from_slice(rrow);
                out.push(composite);
                Ok(())
            };
        for (li, lrow) in left_rows.iter().enumerate() {
            for (ri, rrow) in right_rows.iter().enumerate() {
                let l = &lrow[left_pos];
                let r = &rrow[right_pos];
                if matches!(l, Value::Null) || matches!(r, Value::Null) { continue; }
                if compare_values_for_check(l, r) != std::cmp::Ordering::Equal { continue; }
                left_matched[li] = true;
                right_matched[ri] = true;
                emit(lrow, rrow, &mut composite_rows)?;
            }
        }
        if matches!(kind, JoinKind::LeftOuter | JoinKind::FullOuter) {
            for (li, lrow) in left_rows.iter().enumerate() {
                if !left_matched[li] { emit(lrow, &right_nulls, &mut composite_rows)?; }
            }
        }
        if matches!(kind, JoinKind::RightOuter | JoinKind::FullOuter) {
            for (ri, rrow) in right_rows.iter().enumerate() {
                if !right_matched[ri] { emit(&left_nulls, rrow, &mut composite_rows)?; }
            }
        }
        // Grouped shape dispatch.
        let any_aggregate = items.iter().any(|it| it.is_aggregate());
        let scalar_aggregate: Option<AggSpec> =
            if any_aggregate && items.len() == 1 {
                match &items[0] {
                    SelectItem::Agg(spec) => Some(spec.clone()),
                    _ => None,
                }
            } else { None };
        if group_keys.is_empty() {
            if let Some(spec) = scalar_aggregate {
                let value = evaluate_aggregate(&spec, composite, &composite_rows)?;
                return Ok(SqlResultSet::Rows {
                    columns: vec![spec.result_column().into()],
                    rows: vec![vec![value]],
                });
            }
            return Err(SqlError::Unsupported(
                "joined SELECT aggregates require all projections be aggregates or a GROUP BY list".into()));
        }
        // Validate: each bare projection must be in the key list.
        for item in &items {
            if let SelectItem::Column(Projection::Column(name)) = item {
                if !group_keys.iter().any(|k| k == name) {
                    return Err(SqlError::Unsupported(format!(
                        "projection {name} must appear in GROUP BY")));
                }
            }
        }
        evaluate_grouped(composite, composite_rows, &group_keys, &items, having.as_ref())
    }

    /// ADR 0229 — eagerly resolve every top-level
    /// `IN (SELECT …)` / `NOT IN (SELECT …)` subquery by
    /// executing it against this engine and rewriting the SQL
    /// with a literal list. Non-correlated only; the subquery
    /// must return exactly one column.
    fn resolve_in_subqueries(
        &self, sql: &str, control: &ScanControl,
    ) -> SqlResult<String> {
        let mut out = sql.to_string();
        loop {
            let bytes = out.as_bytes();
            let Some((paren_open, _select_start)) = find_in_subquery(bytes, 0) else { break };
            let close = find_matching_paren(bytes, paren_open)
                .ok_or_else(|| SqlError::Syntax(
                    "IN (SELECT …) subquery has unbalanced parens".into()))?;
            let inner = &out[paren_open + 1..close];
            // Recursive call so nested subqueries resolve too;
            // the recursion terminates because each pass
            // replaces one subquery with a literal list.
            let rs = self.execute_impl(inner, control, None, None, None)?;
            let values = match rs {
                SqlResultSet::Rows { columns, rows } => {
                    if columns.len() != 1 {
                        return Err(SqlError::Unsupported(
                            "IN (SELECT …) subquery must return exactly one column".into()));
                    }
                    rows.into_iter().map(|r| r.into_iter().next().unwrap()).collect::<Vec<_>>()
                }
                _ => return Err(SqlError::Unsupported(
                    "IN (SELECT …) subquery must be a SELECT".into())),
            };
            let list = if values.is_empty() {
                // Empty subquery → IN with impossible sentinel so
                // no row matches. Use a NULL literal which the
                // IN matcher treats as never-match.
                "NULL".to_string()
            } else { format_in_list(&values)? };
            let mut rewritten = String::with_capacity(out.len() + list.len());
            rewritten.push_str(&out[..paren_open + 1]);
            rewritten.push_str(&list);
            rewritten.push_str(&out[close..]);
            out = rewritten;
        }
        Ok(out)
    }

    fn execute_impl_single(
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
                    // ADR 0249 — CREATE USER <name> WITH PASSWORD '<pw>'
                    "user" => {
                        let name = p.word()?;
                        p.expect("with")?;
                        p.expect("password")?;
                        let password = match p.literal()? {
                            Literal::Text(s) => s,
                            _ => return Err(SqlError::Syntax(
                                "CREATE USER requires a single-quoted password literal".into())),
                        };
                        p.end()?;
                        self.users.create_user(&name, &password)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
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
                // ADR 0224 — INSERT INTO t (cols) SELECT a, b
                // FROM src [WHERE pk = lit AND …]. The source
                // SELECT is a narrow MVP subset: bare columns,
                // one source table, optional PK WHERE.
                if p.consume_word("select") {
                    let mut src_cols: Vec<String> = vec![p.word()?];
                    while p.consume_symbol(',') { src_cols.push(p.word()?); }
                    p.expect("from")?;
                    let src_name = p.name()?;
                    let src_table = self.resolve(&src_name)?;
                    let where_clause = if p.next_is_word("where") {
                        Some(parse_select_where(&mut p, &src_table)?)
                    } else { None };
                    let returning = parse_returning(&mut p, &table)?;
                    p.end()?;
                    if src_cols.len() != names.len() || names.len() != table.columns.len() {
                        return Err(SqlError::Unsupported(
                            "INSERT SELECT must project every target column exactly once".into()));
                    }
                    let src_positions: Vec<usize> = src_cols.iter()
                        .map(|c| locate(&src_table, c))
                        .collect::<SqlResult<Vec<_>>>()?;
                    let src_store = self.table_store(&src_table)?;
                    let src_rows: Vec<Vec<Value>> = match &where_clause {
                        Some(SelectWhere::KeyPredicate(key)) =>
                            src_store.get(key)?.into_iter().collect(),
                        Some(SelectWhere::JsonPathPredicates(preds)) =>
                            apply_where_filter(src_store.scan()?, &src_table, preds)?,
                        None => src_store.scan()?,
                    };
                    let target_store = self.table_store(&table)?;
                    let mut unique_pool: Vec<Vec<Value>> = if !table.unique_keys.is_empty() {
                        target_store.scan()?
                    } else { Vec::new() };
                    let mut captured: Vec<Vec<Value>> = Vec::new();
                    let mut inserted_bytes: u64 = 0;
                    for src_row in src_rows {
                        // Project source → ordered target values.
                        let mut fields = BTreeMap::new();
                        for (name_, pos) in names.iter().zip(&src_positions) {
                            if fields.insert(name_.clone(), src_row[*pos].clone()).is_some() {
                                return Err(SqlError::Syntax(
                                    format!("repeated INSERT column {name_}")));
                            }
                        }
                        let values: Vec<Value> = table.columns.iter().map(|column| {
                            let v = fields.remove(&column.name)
                                .ok_or_else(|| SqlError::Resolution(format!(
                                    "missing column {}", column.name)))?;
                            // Type-match: cross-type numerics
                            // (Int32 ↔ Int64) are permitted since
                            // the source may differ in width.
                            match (&column.data_type, &v) {
                                (DataType::Int32, Value::Int64(n)) =>
                                    Ok(Value::Int32((*n).try_into()
                                        .map_err(|_| SqlError::Type(
                                            "INSERT SELECT: value does not fit Int32".into()))?)),
                                (DataType::Int64, Value::Int32(n)) =>
                                    Ok(Value::Int64(i64::from(*n))),
                                _ => Ok(v),
                            }
                        }).collect::<SqlResult<Vec<_>>>()?;
                        check_row(&table, &values)?;
                        foreign_key_check(self, &table, &values)?;
                        if !table.unique_keys.is_empty() {
                            crate::unique_check::UniqueConstraintChecker::new(&table)
                                .check(&values, unique_pool.iter().cloned())?;
                        }
                        let row_bytes = crate::query_budget::row_bytes(&values);
                        target_store.insert(&values)?;
                        inserted_bytes += row_bytes;
                        if !table.unique_keys.is_empty() {
                            unique_pool.push(values.clone());
                        }
                        captured.push(values);
                    }
                    if let Some(b) = budget.as_mut() {
                        b.charge_rows(captured.len() as u64)?;
                        b.charge_bytes(inserted_bytes)?;
                    }
                    return finish_dml_result_with_table(&table, captured, returning.as_deref());
                }
                p.expect("values")?;
                // ADR 0220 — accept one or more comma-separated
                // `(lit, lit, …)` tuples. Each tuple must match
                // the column list.
                let mut tuples: Vec<Vec<Literal>> = Vec::new();
                loop {
                    tuples.push(p.list(|parser| parser.literal())?);
                    if !p.consume_symbol(',') { break; }
                }
                // ADR 0223 — optional ON CONFLICT clause.
                let on_conflict = parse_on_conflict(&mut p, &table)?;
                // ADR 0221 — optional RETURNING tail.
                let returning = parse_returning(&mut p, &table)?;
                p.end()?;
                if names.len() != table.columns.len() {
                    return Err(SqlError::Unsupported(
                        "INSERT must list all table columns once".into()));
                }
                for lits in &tuples {
                    if lits.len() != names.len() {
                        return Err(SqlError::Unsupported(
                            "every INSERT VALUES tuple must match the column list".into()));
                    }
                }
                // Build and coerce every candidate row before
                // touching storage so a bad literal in tuple #17
                // does not leave rows #1..#16 committed.
                let mut rows: Vec<Vec<Value>> = Vec::with_capacity(tuples.len());
                for lits in tuples {
                    let mut fields = BTreeMap::new();
                    for (field, lit) in names.iter().cloned().zip(lits) {
                        if fields.insert(field.clone(), lit).is_some() {
                            return Err(SqlError::Syntax(
                                format!("repeated INSERT column {field}")));
                        }
                        locate(&table, &field)?;
                    }
                    let values = table.columns.iter().map(|column| {
                        let literal = fields.remove(&column.name)
                            .ok_or_else(|| SqlError::Resolution(format!(
                                "missing column {}", column.name)))?;
                        coerce(&literal, column)
                    }).collect::<SqlResult<Vec<_>>>()?;
                    rows.push(values);
                }
                // ADR 0151 — preflight the whole batch before
                // touching storage.
                let batch_bytes: u64 = rows.iter()
                    .map(|r| crate::query_budget::row_bytes(r))
                    .sum();
                if let Some(b) = budget.as_deref() {
                    b.preview_charge(rows.len() as u64, batch_bytes)?;
                }
                let store = self.table_store(&table)?;
                // ADR 0184 — pre-materialize the current store
                // for UNIQUE checks so later rows see earlier
                // ones in the same batch.
                let mut unique_pool: Vec<Vec<Value>> = if !table.unique_keys.is_empty() {
                    store.scan()?
                } else { Vec::new() };
                let mut captured: Vec<Vec<Value>> = Vec::new();
                let mut inserted_bytes: u64 = 0;
                let pk_positions: Vec<usize> = table.primary_key.columns.iter()
                    .map(|c| locate(&table, c))
                    .collect::<SqlResult<Vec<_>>>()?;
                for values in rows {
                    // ADR 0223 — ON CONFLICT: detect existing PK
                    // match and skip / update as directed.
                    if let Some(action) = &on_conflict {
                        let pk: Vec<Value> = pk_positions.iter()
                            .map(|p| values[*p].clone()).collect();
                        if let Some(mut existing) = store.get(&pk)? {
                            match action {
                                OnConflict::DoNothing => continue,
                                OnConflict::DoUpdate { assignments } => {
                                    for (col, lit) in assignments {
                                        let idx = locate(&table, col)?;
                                        existing[idx] = coerce(lit, &table.columns[idx])?;
                                    }
                                    check_row(&table, &existing)?;
                                    foreign_key_check(self, &table, &existing)?;
                                    if !table.unique_keys.is_empty() {
                                        let others = store.scan()?.into_iter()
                                            .filter(|r| pk_positions.iter().zip(pk.iter())
                                                .any(|(p_, k)| &r[*p_] != k));
                                        crate::unique_check::UniqueConstraintChecker::new(&table)
                                            .check(&existing, others)?;
                                    }
                                    let row_bytes = crate::query_budget::row_bytes(&existing);
                                    store.replace(&existing)?;
                                    inserted_bytes += row_bytes;
                                    captured.push(existing);
                                    continue;
                                }
                            }
                        }
                    }
                    check_row(&table, &values)?;
                    foreign_key_check(self, &table, &values)?;
                    if !table.unique_keys.is_empty() {
                        crate::unique_check::UniqueConstraintChecker::new(&table)
                            .check(&values, unique_pool.iter().cloned())?;
                    }
                    let row_bytes = crate::query_budget::row_bytes(&values);
                    store.insert(&values)?;
                    inserted_bytes += row_bytes;
                    if !table.unique_keys.is_empty() {
                        unique_pool.push(values.clone());
                    }
                    captured.push(values);
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(captured.len() as u64)?;
                    b.charge_bytes(inserted_bytes)?;
                }
                finish_dml_result_with_table(&table, captured, returning.as_deref())
            }
            "select" => {
                control.check()?;
                // ADR 0144 — deadline check before scan.
                if let Some(b) = budget.as_deref() { b.check_deadline()?; }
                // ADR 0219 — SELECT DISTINCT dedups the projected
                // output tuple. Rejects composition with aggregates,
                // GROUP BY / HAVING, or ORDER BY / LIMIT / OFFSET
                // for the MVP (each of those is a separate slice
                // waiting on a result-shape model that survives the
                // dedup + ordering pipeline interaction).
                let distinct = p.consume_word("distinct");
                // ADR 0213/0214/0217 — projection list accepts
                // bare columns, JSON paths, and scalar aggregates.
                // Scalar-only aggregate shapes short-circuit later;
                // GROUP BY engages the grouped evaluator.
                let caller_wildcard = p.consume_symbol('*');
                let select_items: Vec<SelectItem> = if caller_wildcard { Vec::new() } else {
                    let mut out = vec![parse_select_item(&mut p)?];
                    while p.consume_symbol(',') { out.push(parse_select_item(&mut p)?); }
                    out
                };
                p.expect("from")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                // ADR 0227 — optional table alias and then
                // [LEFT|RIGHT|FULL [OUTER]] JOIN.
                let left_alias = {
                    if p.consume_word("as") { p.word()? }
                    else if let Some(Token::Word(w)) = p.peek().cloned() {
                        // Bareword alias allowed when it is not
                        // one of the SELECT keywords that follow.
                        let w_low = w.to_ascii_lowercase();
                        let is_keyword = matches!(w_low.as_str(),
                            "where" | "group" | "order" | "limit" | "offset"
                            | "left" | "right" | "full" | "inner" | "join"
                            | "having" | "union");
                        if is_keyword { table.name.clone() }
                        else { p.advance(); w }
                    } else { table.name.clone() }
                };
                let kind = if p.consume_word("left") {
                    p.consume_word("outer");
                    Some(JoinKind::LeftOuter)
                } else if p.consume_word("right") {
                    p.consume_word("outer");
                    Some(JoinKind::RightOuter)
                } else if p.consume_word("full") {
                    p.consume_word("outer");
                    Some(JoinKind::FullOuter)
                } else if p.consume_word("inner") || p.next_is_word("join") {
                    Some(JoinKind::Inner)
                } else { None };
                if let Some(kind) = kind {
                    p.expect("join")?;
                    let right_name = p.name()?;
                    let right_table = self.resolve(&right_name)?;
                    let right_alias = {
                        if p.consume_word("as") { p.word()? }
                        else if let Some(Token::Word(w)) = p.peek().cloned() {
                            let w_low = w.to_ascii_lowercase();
                            let is_keyword = matches!(w_low.as_str(), "on");
                            if is_keyword { right_table.name.clone() }
                            else { p.advance(); w }
                        } else { right_table.name.clone() }
                    };
                    p.expect("on")?;
                    let left_ref = p.name()?;
                    p.symbol('=')?;
                    let right_ref = p.name()?;
                    let where_clause = if p.next_is_word("where") {
                        p.expect("where")?;
                        let col = p.name()?;
                        p.symbol('=')?;
                        let lit = p.literal()?;
                        Some((col, lit))
                    } else { None };
                    // ADR 0228 — parse GROUP BY / HAVING against
                    // a synthetic composite table built from the
                    // join's aliased columns.
                    let composite = build_composite_table(
                        &table, &left_alias, &right_table, &right_alias);
                    let group_keys = parse_group_by(&mut p, &composite)?;
                    let having = parse_having(&mut p, &composite, &group_keys)?;
                    p.end()?;
                    return self.execute_inner_join(
                        kind,
                        &table, &left_alias,
                        &right_table, &right_alias,
                        &left_ref, &right_ref,
                        &select_items, caller_wildcard,
                        where_clause,
                        group_keys, having,
                        &composite,
                        control, budget,
                    );
                }
                // ADR 0217 — expand `SELECT *` into bare columns.
                let mut select_items: Vec<SelectItem> = if select_items.is_empty() {
                    table.columns.iter()
                        .map(|c| SelectItem::Column(Projection::Column(c.name.clone())))
                        .collect()
                } else { select_items };
                if select_items.is_empty() {
                    return Err(SqlError::Syntax("SELECT requires columns".into()));
                }
                // ADR 0226 — reject dotted projections on the
                // single-table path.
                for item in &select_items {
                    if let SelectItem::Column(Projection::Column(name)) = item {
                        if name.contains('.') {
                            return Err(SqlError::Unsupported(
                                "qualified column references require a JOIN".into()));
                        }
                    }
                }
                // Back-compat for existing rows path: strip into
                // projections when no aggregate appears.
                let any_aggregate = select_items.iter().any(|it| it.is_aggregate());
                let scalar_aggregate: Option<AggSpec> =
                    if any_aggregate && select_items.len() == 1 {
                        match &select_items[0] {
                            SelectItem::Agg(spec) => Some(spec.clone()),
                            _ => None,
                        }
                    } else { None };
                // ADR 0155 — column-level policy filtering on
                // bare-column items only. Aggregate projections
                // bypass the column-level filter because they
                // compute scalar outputs, not raw cells.
                if let Some(p_) = policy {
                    if p_.column_level.is_configured() {
                        if caller_wildcard {
                            select_items.retain(|item| match item {
                                SelectItem::Column(pj) => p_.column_level.allows_read(pj.source_column()),
                                SelectItem::Agg(_) | SelectItem::Func(_) => true,
                            });
                        } else {
                            for item in &select_items {
                                if let SelectItem::Column(pj) = item {
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
                }
                // ADR 0190 — WHERE accepts PK equality or
                // JSON-path equality (full-scan post-filter).
                let where_clause = if p.next_is_word("where") {
                    Some(parse_select_where(&mut p, &table)?)
                } else { None };
                // ADR 0217 — GROUP BY, parsed between WHERE and
                // ORDER BY / LIMIT / OFFSET.
                let group_keys = parse_group_by(&mut p, &table)?;
                let having = parse_having(&mut p, &table, &group_keys)?;
                // ADR 0219 — DISTINCT is orthogonal to the
                // grouping / aggregation pipeline.
                if distinct && (any_aggregate || !group_keys.is_empty()) {
                    return Err(SqlError::Unsupported(
                        "DISTINCT cannot be combined with aggregates or GROUP BY".into()));
                }
                // ADR 0217 — require every bare projection to be
                // a listed group key when grouping is active.
                if !group_keys.is_empty() || (any_aggregate && scalar_aggregate.is_none()) {
                    if group_keys.is_empty() {
                        return Err(SqlError::Unsupported(
                            "mixing aggregate and bare projections requires GROUP BY".into()));
                    }
                    for item in &select_items {
                        if let SelectItem::Column(Projection::Column(name)) = item {
                            if !group_keys.iter().any(|k| k == name) {
                                return Err(SqlError::Unsupported(format!(
                                    "projection {name} must appear in GROUP BY")));
                            }
                        } else if let SelectItem::Column(Projection::JsonPath { .. }) = item {
                            return Err(SqlError::Unsupported(
                                "GROUP BY with JSON-path projection is not supported".into()));
                        }
                    }
                }
                // ADR 0212 — ORDER BY / LIMIT / OFFSET on the
                // base engine, reusing the historical window.
                let window = crate::select_window::SelectWindow::parse(&mut p, &table)?;
                p.end()?;
                // ADR 0217 — materialize legacy `projections` /
                // `indices` only for the plain-row path.
                let plain_projections: Vec<Projection> = select_items.iter().filter_map(|item|
                    if let SelectItem::Column(pj) = item { Some(pj.clone()) } else { None }
                ).collect();
                let indices: Vec<usize> = plain_projections.iter()
                    .map(|pj| locate(&table, pj.source_column()))
                    .collect::<SqlResult<Vec<_>>>()?;
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
                // ADR 0213/0214/0215/0216 — scalar aggregate
                // (single agg projection, no GROUP BY) collapses
                // the filtered scan into a single-row result.
                if let Some(spec) = scalar_aggregate {
                    let value = evaluate_aggregate(&spec, &table, &all)?;
                    return Ok(SqlResultSet::Rows {
                        columns: vec![spec.result_column().into()],
                        rows: vec![vec![value]],
                    });
                }
                // ADR 0217 — grouped evaluation: bucket by group
                // key, evaluate per-bucket. ORDER BY / LIMIT /
                // OFFSET parse but are discarded (results are
                // emitted sorted by group key).
                if !group_keys.is_empty() {
                    return evaluate_grouped(&table, all, &group_keys, &select_items, having.as_ref());
                }
                // ADR 0212 — apply ORDER BY / LIMIT / OFFSET
                // before projection so sort keys can reference
                // columns that are not in the projection list.
                let all = window.apply(all, &table)?;
                // ADR 0232 — if any projection is a scalar
                // function, evaluate per full row (functions
                // can reference columns outside the projection
                // list via column name).
                let any_func = select_items.iter().any(|it| it.is_func());
                let rows: Vec<Vec<Value>> = if any_func {
                    let _ = indices;
                    all.into_iter().map(|row|
                        select_items.iter().map(|item| match item {
                            SelectItem::Column(pj) => {
                                let pos = locate(&table, pj.source_column())?;
                                apply_projection(pj, row[pos].clone())
                            }
                            SelectItem::Func(fe) => evaluate_func(fe, &row, &table),
                            SelectItem::Agg(_) => unreachable!("scalar row path"),
                        }).collect::<SqlResult<Vec<Value>>>()
                    ).collect::<SqlResult<Vec<Vec<Value>>>>()?
                } else {
                    let base_rows = crate::typed_table::project_rows_with_control(all, &indices, control)?;
                    base_rows.into_iter().map(|row|
                        row.into_iter().zip(&plain_projections).map(|(value, pj)| apply_projection(pj, value))
                            .collect::<SqlResult<Vec<Value>>>()
                    ).collect::<SqlResult<Vec<Vec<Value>>>>()?
                };
                let rows = if distinct {
                    let mut out = dedup_rows(rows);
                    sort_distinct_output(&mut out);
                    out
                } else { rows };
                let columns: Vec<String> = select_items.iter().map(|it| it.label()).collect();
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
                // ADR 0221 — optional RETURNING tail.
                let returning = parse_returning(&mut p, &table)?;
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
                // ADR 0221 — capture mutated rows for RETURNING.
                let mut captured: Vec<Vec<Value>> = Vec::new();
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
                    captured.push(row);
                    affected += 1;
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(affected)?;
                    if affected > 0 { b.charge_bytes(written_bytes)?; }
                }
                finish_dml_result_with_table(&table, captured, returning.as_deref())
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
                // ADR 0221 — optional RETURNING tail captures
                // rows BEFORE they are removed.
                let returning = parse_returning(&mut p, &table)?;
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
                // ADR 0221 — capture deleted rows for RETURNING.
                let mut captured: Vec<Vec<Value>> = Vec::new();
                for (pk, row) in targets {
                    let row_bytes = crate::query_budget::row_bytes(&row);
                    if let Some(b) = budget.as_deref() {
                        b.preview_charge(1, row_bytes)?;
                    }
                    foreign_key_restrict(self, &table, &pk)?;
                    store.delete(&pk)?;
                    deleted_bytes = deleted_bytes.saturating_add(row_bytes);
                    captured.push(row);
                    affected += 1;
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(affected)?;
                    if affected > 0 { b.charge_bytes(deleted_bytes)?; }
                }
                finish_dml_result_with_table(&table, captured, returning.as_deref())
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
                // ADR 0222 — optional RETURNING captures rows
                // before bulk deletion.
                let returning = parse_returning(&mut p, &table)?;
                p.end()?;
                let store = self.table_store(&table)?;
                let rows = store.scan()?;
                let truncated_bytes = crate::query_budget::rows_bytes(&rows);
                if let Some(b) = budget.as_deref() {
                    b.preview_charge(rows.len() as u64, truncated_bytes)?;
                }
                let mut captured: Vec<Vec<Value>> = Vec::new();
                for row in rows {
                    let pk_values: Vec<Value> = table.primary_key.columns.iter()
                        .map(|c| {
                            let idx = table.columns.iter()
                                .position(|col| col.name == *c).unwrap();
                            row[idx].clone()
                        }).collect();
                    store.delete(&pk_values)?;
                    captured.push(row);
                }
                if let Some(b) = budget.as_mut() {
                    b.charge_rows(captured.len() as u64)?;
                    b.charge_bytes(truncated_bytes)?;
                }
                finish_dml_result_with_table(&table, captured, returning.as_deref())
            }
            // ADR 0165/0166 — ALTER TABLE statements.
            //   ALTER TABLE <name> RENAME TO <new_name>
            //   ALTER TABLE <name> RENAME COLUMN <old> TO <new>
            //   ALTER TABLE <name> DROP COLUMN <name>
            //   ALTER TABLE <name> ALTER COLUMN <col> SET DEFAULT <literal>
            //   ALTER TABLE <name> ALTER COLUMN <col> DROP DEFAULT
            //   ALTER TABLE <name> ALTER COLUMN <col> TYPE <type>
            "alter" => {
                // ADR 0249 — ALTER USER <name> SET PASSWORD '<pw>'
                //            ALTER USER <name> SET ACTIVE { TRUE | FALSE }
                if p.consume_word("user") {
                    let user_name = p.word()?;
                    p.expect("set")?;
                    if p.consume_word("password") {
                        let password = match p.literal()? {
                            Literal::Text(s) => s,
                            _ => return Err(SqlError::Syntax(
                                "ALTER USER SET PASSWORD requires a single-quoted password literal".into())),
                        };
                        p.end()?;
                        self.users.set_password(&user_name, &password)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
                    if p.consume_word("active") {
                        let active = if p.consume_word("true") { true }
                            else if p.consume_word("false") { false }
                            else { return Err(SqlError::Syntax(
                                "ALTER USER SET ACTIVE expects TRUE or FALSE".into())); };
                        p.end()?;
                        self.users.set_active(&user_name, active)?;
                        return Ok(SqlResultSet::Command { affected: 0 });
                    }
                    return Err(SqlError::Syntax(
                        "ALTER USER expects SET PASSWORD or SET ACTIVE".into()));
                }
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
                        // ADR 0241 — SET NOT NULL [BACKFILL lit].
                        if p.consume_word("not") {
                            p.expect("null")?;
                            let backfill_literal = if p.consume_word("backfill") {
                                Some(p.literal()?)
                            } else { None };
                            p.end()?;
                            let column = table.columns.iter()
                                .find(|c| c.name == col)
                                .ok_or_else(|| SqlError::Resolution(
                                    format!("unknown column {col}")))?;
                            if !column.nullable {
                                return Err(SqlError::Unsupported(
                                    "SET NOT NULL: column is already NOT NULL".into()));
                            }
                            let col_pos = locate(&table, &col)?;
                            let mut store = self.table_store(&table)?;
                            let existing = store.scan()?;
                            let has_null = existing.iter().any(|row|
                                matches!(row[col_pos], Value::Null));
                            let backfill_value = match (has_null, backfill_literal) {
                                (false, _) => None,
                                (true, None) => return Err(SqlError::Type(
                                    "SET NOT NULL rejects: existing rows carry NULL and no BACKFILL was supplied".into())),
                                (true, Some(lit)) => Some(coerce(&lit, column)?),
                            };
                            if let Some(ref v) = backfill_value {
                                for mut row in existing {
                                    if matches!(row[col_pos], Value::Null) {
                                        row[col_pos] = v.clone();
                                        store.replace(&row)?;
                                    }
                                }
                            }
                            self.catalog.strengthen_column_not_null(table.id, &col)?;
                            // ADR 0241 — rewrite the META_KEY so the
                            // next open sees the strengthened schema.
                            let new_table = self.catalog.snapshot()
                                .table(table.id)
                                .ok_or_else(|| SqlError::Resolution(
                                    "table disappeared after SET NOT NULL".into()))?
                                .clone();
                            store.rewrite_schema_pin(new_table)
                                .map_err(|e| SqlError::Type(format!(
                                    "SET NOT NULL pin rewrite: {e}")))?;
                            return Ok(SqlResultSet::Command { affected: 0 });
                        }
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
            // ADR 0249 — DROP USER <name>
            "drop" => {
                p.expect("user")?;
                let name = p.word()?;
                p.end()?;
                self.users.drop_user(&name)?;
                Ok(SqlResultSet::Command { affected: 0 })
            }
            // ADR 0251 — GRANT <cap> TO <user>
            "grant" => {
                let cap = parse_capability(&mut p)?;
                p.expect("to")?;
                let user = p.word()?;
                p.end()?;
                self.users.grant(&user, cap)?;
                Ok(SqlResultSet::Command { affected: 0 })
            }
            // ADR 0251 — REVOKE <cap> FROM <user>
            "revoke" => {
                let cap = parse_capability(&mut p)?;
                p.expect("from")?;
                let user = p.word()?;
                p.end()?;
                self.users.revoke(&user, cap)?;
                Ok(SqlResultSet::Command { affected: 0 })
            }
            _ => Err(SqlError::Unsupported(format!("statement {action}"))),
        }
    }
}


/// ADR 0250 — a SqlEngine wrapper bound to a principal.
/// Every `execute` applies the principal's AuthorizationContext
/// before dispatching through the underlying engine. The
/// principal is immutable for the lifetime of the session —
/// callers wanting a different identity must open a new
/// session via `SqlEngine::with_authenticated_session`.
pub struct AuthenticatedSession<'a> {
    engine: &'a SqlEngine,
    authz: bradian_core::AuthorizationContext,
}

impl<'a> AuthenticatedSession<'a> {
    /// The verified principal for this session.
    pub fn principal(&self) -> &str { self.authz.principal_id() }

    /// The capability bundle the principal may exercise.
    pub fn capabilities(&self) -> &[bradian_core::Capability] {
        self.authz.capabilities()
    }

    /// Run a SQL statement through the principal's authz
    /// context. A missing capability fast-fails as
    /// `SqlError::Authz` before the parser runs.
    pub fn execute(&self, sql: &str) -> SqlResult<SqlResultSet> {
        self.engine.execute_as(&self.authz, sql)
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
