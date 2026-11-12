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
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use thiserror::Error;

use crate::{Column, DataType, PrimaryKey, Table, TableSpec};
use crate::persistent_catalog::{PersistentCatalog, PersistentCatalogError};
use crate::typed_table::{TableError, TableStore, Value};

#[derive(Debug, Error)]
pub enum SqlError {
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
    Io(#[from] std::io::Error),
}

pub type SqlResult<T> = Result<T, SqlError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlResultSet {
    Command { affected: u64 },
    Rows { columns: Vec<String>, rows: Vec<Vec<Value>> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Number(String),
    Text(String),
    Symbol(char),
}

fn lex(source: &str) -> SqlResult<Vec<Token>> {
    let mut chars = source.chars().peekable();
    let mut out = Vec::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_whitespace() { chars.next(); continue; }
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
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            let mut word = String::new();
            while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_') {
                word.push(chars.next().unwrap());
            }
            out.push(Token::Word(word));
        } else if ch.is_ascii_digit() || ch == '-' {
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
        } else if "(),.=;*".contains(ch) {
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
enum Literal {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(sql: &str) -> SqlResult<Self> {
        Ok(Self { tokens: lex(sql)?, pos: 0 })
    }

    fn peek(&self) -> Option<&Token> { self.tokens.get(self.pos) }

    fn word(&mut self) -> SqlResult<String> {
        match self.tokens.get(self.pos).cloned() {
            Some(Token::Word(word)) => {
                self.pos += 1;
                Ok(word.to_ascii_lowercase())
            }
            _ => Err(SqlError::Syntax("expected an unquoted identifier".into())),
        }
    }

    fn consume_word(&mut self, value: &str) -> bool {
        if matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(value)) {
            self.pos += 1;
            true
        } else { false }
    }

    fn expect(&mut self, value: &str) -> SqlResult<()> {
        if self.consume_word(value) { Ok(()) }
        else { Err(SqlError::Syntax(format!("expected {value}"))) }
    }

    fn consume_symbol(&mut self, value: char) -> bool {
        if self.peek() == Some(&Token::Symbol(value)) {
            self.pos += 1;
            true
        } else { false }
    }

    fn symbol(&mut self, value: char) -> SqlResult<()> {
        if self.consume_symbol(value) { Ok(()) }
        else { Err(SqlError::Syntax(format!("expected '{value}'"))) }
    }

    fn name(&mut self) -> SqlResult<Vec<String>> {
        let mut parts = vec![self.word()?];
        while self.consume_symbol('.') {
            parts.push(self.word()?);
            if parts.len() > 3 {
                return Err(SqlError::Unsupported("identifiers have at most three parts".into()));
            }
        }
        Ok(parts)
    }

    fn list<T>(&mut self, mut parse: impl FnMut(&mut Self) -> SqlResult<T>) -> SqlResult<Vec<T>> {
        self.symbol('(')?;
        let mut values = Vec::new();
        loop {
            values.push(parse(self)?);
            if !self.consume_symbol(',') { break; }
        }
        self.symbol(')')?;
        Ok(values)
    }

    fn literal(&mut self) -> SqlResult<Literal> {
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

    fn number_u32(&mut self) -> SqlResult<u32> {
        match self.tokens.get(self.pos).cloned() {
            Some(Token::Number(value)) if !value.contains('.') && !value.starts_with('-') => {
                self.pos += 1;
                value.parse().map_err(|_| SqlError::Syntax("invalid positive integer".into()))
            }
            _ => Err(SqlError::Syntax("expected nonnegative integer".into())),
        }
    }

    fn data_type(&mut self) -> SqlResult<DataType> {
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
            _ => Err(SqlError::Unsupported(format!("type {name} is not in the G12 SQL profile"))),
        }
    }

    fn assignments(&mut self) -> SqlResult<Vec<(String, Literal)>> {
        let mut fields = Vec::new();
        loop {
            let column = self.word()?;
            self.symbol('=')?;
            fields.push((column, self.literal()?));
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

    fn end(&mut self) -> SqlResult<()> {
        self.consume_symbol(';');
        if self.pos == self.tokens.len() { Ok(()) }
        else { Err(SqlError::Unsupported("trailing tokens or multiple statements".into())) }
    }
}

fn qualified(parts: &[String], expected: usize) -> SqlResult<()> {
    if parts.len() == expected { Ok(()) }
    else { Err(SqlError::Resolution(format!("expected {expected}-part qualified name"))) }
}

fn coerce(lit: &Literal, column: &Column) -> SqlResult<Value> {
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

fn locate(table: &Table, field: &str) -> SqlResult<usize> {
    table.columns.iter().position(|c| c.name == field)
        .ok_or_else(|| SqlError::Resolution(format!("unknown column {field}")))
}

fn key_predicate(
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
}

impl SqlEngine {
    pub fn open(root: impl AsRef<Path>) -> SqlResult<Self> {
        std::fs::create_dir_all(root.as_ref())?;
        std::fs::create_dir_all(root.as_ref().join("tables"))?;
        let catalog = PersistentCatalog::open(root.as_ref().join("catalog"))?;
        Ok(Self { root: root.as_ref().to_owned(), catalog, operation: Mutex::new(()) })
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
        Ok(TableStore::open(self.root.join("tables").join(table.id.0.to_string()), table.clone())?)
    }

    pub fn execute(&self, sql: &str) -> SqlResult<SqlResultSet> {
        let _guard = self.operation.lock();
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
                        p.symbol('(')?;
                        loop {
                            if p.consume_word("primary") {
                                p.expect("key")?;
                                if !primary.is_empty() {
                                    return Err(SqlError::Syntax("multiple PRIMARY KEY definitions".into()));
                                }
                                primary = p.list(|parser| parser.word())?;
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
                        self.catalog.create_table(schema.id, TableSpec {
                            name: name[2].clone(), columns,
                            primary_key: PrimaryKey { columns: primary },
                            unique_keys: vec![],
                        })?;
                        // TableStore is provisioned on first use. Catalog and
                        // user-table directories are not one atomic DDL commit.
                    }
                    _ => return Err(SqlError::Unsupported(format!("CREATE {kind}"))),
                }
                Ok(SqlResultSet::Command { affected: 0 })
            }
            "insert" => {
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
                self.table_store(&table)?.insert(&values)?;
                Ok(SqlResultSet::Command { affected: 1 })
            }
            "select" => {
                let selected = if p.consume_symbol('*') { Vec::new() } else {
                    let mut names = vec![p.word()?];
                    while p.consume_symbol(',') { names.push(p.word()?); }
                    names
                };
                p.expect("from")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                let mut selected = if selected.is_empty() {
                    table.columns.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
                } else { selected };
                if selected.is_empty() {
                    return Err(SqlError::Syntax("SELECT requires columns".into()));
                }
                let mut indices = Vec::new();
                for column in &selected { indices.push(locate(&table, column)?); }
                let store = self.table_store(&table)?;
                let all = if p.consume_word("where") {
                    p.pos -= 1;
                    let pk = key_predicate(&mut p, &table)?;
                    store.get(&pk)?.into_iter().collect::<Vec<_>>()
                } else { store.scan()? };
                p.end()?;
                let rows = all.into_iter().map(|row| {
                    indices.iter().map(|index| row[*index].clone()).collect()
                }).collect();
                Ok(SqlResultSet::Rows { columns: std::mem::take(&mut selected), rows })
            }
            "update" => {
                let name = p.name()?;
                let table = self.resolve(&name)?;
                p.expect("set")?;
                let changes = p.assignments()?;
                let pk = key_predicate(&mut p, &table)?;
                p.end()?;
                let mut assignments = BTreeMap::new();
                for (field, lit) in changes {
                    let position = locate(&table, &field)?;
                    if table.primary_key.columns.contains(&field) {
                        return Err(SqlError::Unsupported("UPDATE cannot modify primary-key columns".into()));
                    }
                    if assignments.insert(position, coerce(&lit, &table.columns[position])?).is_some() {
                        return Err(SqlError::Syntax("duplicate SET column".into()));
                    }
                }
                let store = self.table_store(&table)?;
                let affected = if let Some(mut row) = store.get(&pk)? {
                    for (pos, value) in assignments { row[pos] = value; }
                    store.replace(&row)?;
                    1
                } else { 0 };
                Ok(SqlResultSet::Command { affected })
            }
            "delete" => {
                p.expect("from")?;
                let name = p.name()?;
                let table = self.resolve(&name)?;
                let pk = key_predicate(&mut p, &table)?;
                p.end()?;
                let store = self.table_store(&table)?;
                let affected = if store.get(&pk)?.is_some() {
                    store.delete(&pk)?;
                    1
                } else { 0 };
                Ok(SqlResultSet::Command { affected })
            }
            _ => Err(SqlError::Unsupported(format!("statement {action}"))),
        }
    }
}
