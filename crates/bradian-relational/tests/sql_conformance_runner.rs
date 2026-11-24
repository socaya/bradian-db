//! ADR 0277 — SQLLogicTest-compatible runner.
//!
//! Reads `.test` files from
//! `tests/sql_conformance_cases/` and executes each
//! directive against a fresh `SqlEngine`. The dialect is a
//! subset of SQLLogicTest:
//!
//! - `# comment` — line comment.
//! - `statement ok` — next non-blank line is SQL that must succeed.
//! - `statement error` — SQL that must fail (any `SqlError` variant).
//! - `query I` / `query T` — SQL; the `----` line precedes
//!   expected rows; each expected row on its own line.
//!   Type hints are informational only.
//! - Blank lines separate directives.
//!
//! The runner is intentionally minimal — just enough to
//! close the G1-REFERENCE in-tree portion. External suite
//! integration (SQLLogicTest itself, pgTAP) stays open as an
//! infrastructure task.

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;
use tempfile::tempdir;

#[derive(Debug, PartialEq, Eq)]
enum Directive {
    StatementOk(String),
    StatementError(String),
    Query { sql: String, expected: Vec<String> },
}

fn parse_file(path: &std::path::Path) -> Vec<Directive> {
    let text = std::fs::read_to_string(path).unwrap();
    let mut out = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') { continue; }
        if trimmed == "statement ok" {
            let sql = next_sql(&mut lines);
            out.push(Directive::StatementOk(sql));
        } else if trimmed == "statement error" {
            let sql = next_sql(&mut lines);
            out.push(Directive::StatementError(sql));
        } else if let Some(rest) = trimmed.strip_prefix("query ") {
            let _hint = rest;
            let sql = next_sql(&mut lines);
            // Expect `----` then expected rows up to blank line.
            while let Some(peek) = lines.peek() {
                if peek.trim() == "----" { lines.next(); break; }
                if peek.trim().is_empty() || peek.trim().starts_with('#') {
                    lines.next();
                } else {
                    panic!("query without `----` separator before expected rows in {path:?}");
                }
            }
            let mut expected = Vec::new();
            while let Some(peek) = lines.peek() {
                let t = peek.trim();
                if t.is_empty() || t.starts_with("statement ")
                    || t.starts_with("query ")
                    || t == "----"
                { break; }
                if t.starts_with('#') { lines.next(); continue; }
                expected.push(t.to_string());
                lines.next();
            }
            out.push(Directive::Query { sql, expected });
        } else {
            panic!("unknown directive line in {path:?}: {line:?}");
        }
    }
    out
}

fn next_sql<'a, I: Iterator<Item = &'a str>>(lines: &mut std::iter::Peekable<I>) -> String {
    let mut out = String::new();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if t.is_empty() { if !out.is_empty() { break; } continue; }
        if t.starts_with('#') { continue; }
        if !out.is_empty() { out.push('\n'); }
        out.push_str(t);
        if let Some(next) = lines.peek() {
            let nt = next.trim();
            if nt.is_empty() || nt.starts_with("statement ")
                || nt.starts_with("query ") || nt == "----" { break; }
        }
    }
    out
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        Value::Boolean(b) => if *b { "true".into() } else { "false".into() },
        Value::Int32(n) => n.to_string(),
        Value::Int64(n) => n.to_string(),
        Value::Decimal(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Json(s) => s.clone(),
        Value::TimestampTz(n) => n.to_string(),
        Value::Uuid(b) => {
            let mut s = String::with_capacity(36);
            for (i, byte) in b.iter().enumerate() {
                if matches!(i, 4 | 6 | 8 | 10) { s.push('-'); }
                s.push_str(&format!("{byte:02x}"));
            }
            s
        }
    }
}

fn run_file(path: &std::path::Path) {
    let dir = tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for directive in parse_file(path) {
        match directive {
            Directive::StatementOk(sql) => {
                db.execute(&sql).unwrap_or_else(|e|
                    panic!("statement ok in {path:?} failed: {sql}\n  err: {e}"));
            }
            Directive::StatementError(sql) => {
                if db.execute(&sql).is_ok() {
                    panic!("statement error in {path:?} unexpectedly succeeded: {sql}");
                }
            }
            Directive::Query { sql, expected } => {
                let rs = db.execute(&sql).unwrap_or_else(|e|
                    panic!("query in {path:?} failed: {sql}\n  err: {e}"));
                let got_rows: Vec<String> = match rs {
                    SqlResultSet::Rows { rows, .. } => rows.iter()
                        .map(|r| r.iter().map(value_to_string).collect::<Vec<_>>().join(" "))
                        .collect(),
                    other => panic!("expected Rows result, got {other:?}"),
                };
                assert_eq!(got_rows, expected,
                    "query mismatch in {path:?}: {sql}");
            }
        }
    }
}

#[test]
fn run_sql_conformance_cases() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/sql_conformance_cases");
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap()
        .filter_map(|e| {
            let p = e.unwrap().path();
            if p.extension().is_some_and(|e| e == "test") { Some(p) } else { None }
        }).collect();
    files.sort();
    assert!(!files.is_empty(), "no .test files found in {dir:?}");
    for file in &files {
        println!("running {file:?}");
        run_file(file);
    }
}
