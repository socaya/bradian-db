use std::io::Read;
use std::path::{Path, PathBuf};
use anyhow::{Context, Result};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::sql::SqlEngine;
use bradian_relational::runtime_limits::{ScanControl, ScanLimits, HistoryLimits, StagingLimits};
use clap::Parser;
use crate::local_sql_output::{output,DEFAULT_OUTPUT_BYTES};
#[cfg(test)]
use {serde_json::Value as Json,bradian_relational::{typed_table::Value,sql::SqlResultSet}};

const MAX_SCRIPT: usize = 1024 * 1024;
const MAX_STATEMENT: usize = 64 * 1024;
const MAX_STATEMENTS: usize = 256;

#[derive(Parser)]
#[command(group(clap::ArgGroup::new("input").required(true).args(["execute", "file"])))]
pub(crate) struct SqlArgs {
    /// Private local relational root (created with mode 0700 on Unix).
    #[arg(long)]
    root: PathBuf,
    /// SQL statements separated by semicolons.
    #[arg(long)]
    execute: Option<String>,
    /// UTF-8 SQL script, or '-' for standard input.
    #[arg(long)]
    file: Option<PathBuf>,
    /// Run a scoped transaction script against one existing table.
    #[arg(long, conflicts_with = "history")]
    table: Option<String>,
    /// Use an explicitly migrated, existing historical SQL root.
    #[arg(long)]
    history: bool,
    /// SQL scan/snapshot input-row budget (default 100000; ordered queries at most 10000).
    #[arg(long)]
    max_scan_rows: Option<usize>,
    /// SQL encoded/decoded/result buffer budget in bytes (default 16777216 each).
    #[arg(long)]
    max_scan_bytes: Option<usize>,
    /// SQL examined key/version/projection-cell budget (default 1000000).
    #[arg(long)]
    max_scan_work: Option<usize>,
    /// Encoded JSON output bytes per statement, including newline (default 16777216).
    #[arg(long)]
    max_output_bytes: Option<usize>,
    /// Retained logical history key limit (default 1000000).
    #[arg(long, requires = "history")]
    max_history_keys: Option<usize>,
    /// Retained history version limit (default 1000000).
    #[arg(long, requires = "history")]
    max_history_versions: Option<usize>,
    /// Accounted retained history bytes (default 268435456; not total RSS).
    #[arg(long, requires = "history")]
    max_history_bytes: Option<usize>,
    /// Distinct staged history keys (default and physical maximum 256).
    #[arg(long, requires = "history")]
    max_staged_keys: Option<usize>,
    /// Accounted staged mutations plus savepoint copies (default 8388608).
    #[arg(long, requires = "history")]
    max_staged_bytes: Option<usize>,
}

fn read_script(args: &SqlArgs) -> Result<String> {
    if let Some(source) = &args.execute {
        anyhow::ensure!(source.len() <= MAX_SCRIPT, "SQL input exceeds 1 MiB");
        return Ok(source.clone());
    }
    let path = args.file.as_ref().context("SQL input is required")?;
    let source: Box<dyn Read> = if path == Path::new("-") { Box::new(std::io::stdin()) }
        else { Box::new(std::fs::File::open(path).context("cannot open SQL script")?) };
    let mut bytes = Vec::new();
    source.take((MAX_SCRIPT + 1) as u64).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= MAX_SCRIPT, "SQL input exceeds 1 MiB");
    String::from_utf8(bytes).context("SQL input must be UTF-8")
}

fn statements(source: &str) -> Result<Vec<&str>> {
    let bytes = source.as_bytes();
    let mut quoted = false;
    let mut position = 0;
    let mut start = 0;
    let mut result = Vec::new();
    while position < bytes.len() {
        if bytes[position] == b'\'' {
            if quoted && bytes.get(position + 1) == Some(&b'\'') { position += 2; continue; }
            quoted = !quoted;
        } else if !quoted {
            anyhow::ensure!(!matches!((bytes[position], bytes.get(position + 1)),
                (b'-', Some(b'-')) | (b'/', Some(b'*'))), "SQL comments are unsupported");
            if bytes[position] == b';' {
                push_statement(&mut result, &source[start..position])?;
                start = position + 1;
            }
        }
        position += 1;
    }
    anyhow::ensure!(!quoted, "unterminated SQL string literal");
    push_statement(&mut result, &source[start..])?;
    anyhow::ensure!(!result.is_empty(), "SQL script contains no statements");
    Ok(result)
}

fn push_statement<'a>(result: &mut Vec<&'a str>, statement: &'a str) -> Result<()> {
    let statement = statement.trim();
    if statement.is_empty() { return Ok(()); }
    anyhow::ensure!(statement.len() <= MAX_STATEMENT, "SQL statement exceeds 64 KiB");
    anyhow::ensure!(result.len() < MAX_STATEMENTS, "SQL script exceeds 256 statements");
    result.push(statement);
    Ok(())
}

#[cfg(unix)]
fn private_root(root: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    match std::fs::symlink_metadata(root) {
        Ok(metadata) => {
            anyhow::ensure!(!metadata.file_type().is_symlink() && metadata.is_dir(), "SQL root must be a real directory, not a symlink");
            anyhow::ensure!(metadata.permissions().mode() & 0o077 == 0, "SQL root must have no group/other permissions (use a private 0700 directory)");
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true).mode(0o700).create(root)?;
        },
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

#[cfg(not(unix))]
fn private_root(_root: &Path) -> Result<()> {
    anyhow::bail!("native local SQL directory permissions are not yet qualified on this platform")
}

pub(crate) fn run(args: SqlArgs) -> Result<()> {
    let source = read_script(&args)?;
    let statements = statements(&source)?;
    if args.history { anyhow::ensure!(args.root.join("history-domain").is_dir(), "--history requires an existing migrated history-domain root"); }
    private_root(&args.root)?;
    let output_limit=args.max_output_bytes.unwrap_or(DEFAULT_OUTPUT_BYTES);
    let mut writer = std::io::BufWriter::new(std::io::stdout().lock());
    let defaults = ScanLimits::default();
    let control = ScanControl::new(ScanLimits {
        max_rows: args.max_scan_rows.unwrap_or(defaults.max_rows),
        max_bytes: args.max_scan_bytes.unwrap_or(defaults.max_bytes),
        max_work: args.max_scan_work.unwrap_or(defaults.max_work),
    });
    if args.history {
        let defaults = HistoryLimits::default();
        let engine = HistorySqlEngine::open_with_history_limits(&args.root, HistoryLimits {
            max_keys: args.max_history_keys.unwrap_or(defaults.max_keys),
            max_versions: args.max_history_versions.unwrap_or(defaults.max_versions),
            max_bytes: args.max_history_bytes.unwrap_or(defaults.max_bytes),
        })?;
        let defaults = StagingLimits::default();
        engine.with_session_limits(control, StagingLimits {
            max_keys: args.max_staged_keys.unwrap_or(defaults.max_keys),
            max_bytes: args.max_staged_bytes.unwrap_or(defaults.max_bytes),
        }, |session| {
            for (index, statement) in statements.iter().enumerate() {
                let result = session.execute(statement).with_context(||format!("statement {} failed", index + 1));
                // Keep CLI output errors outside engine error categories while
                // returning through the scoped callback so pending work is dropped.
                let result = result.map_err(|e| std::io::Error::other(format!("{e:#}")))?;
                output(&mut writer, index + 1, result, output_limit).map_err(|e|std::io::Error::other(format!("{e:#}")))?;
            }
            if session.in_transaction() { return Err(std::io::Error::other("script ended with an uncommitted transaction; staged work rolled back").into()); }
            Ok(())
        })?;
    } else {
        let engine = SqlEngine::open(&args.root)?;
        if let Some(table) = &args.table {
            engine.with_sql_session_control(table, control, |session| {
                for (index, statement) in statements.iter().enumerate() {
                    let result = session.execute(statement).with_context(||format!("statement {} failed", index + 1))
                        .map_err(|e|std::io::Error::other(format!("{e:#}")))?;
                    output(&mut writer, index + 1, result, output_limit).map_err(|e|std::io::Error::other(format!("{e:#}")))?;
                }
                if session.in_transaction() { return Err(std::io::Error::other("script ended with an uncommitted transaction; staged work rolled back").into()); }
                Ok(())
            })?;
        } else {
            for (index, statement) in statements.iter().enumerate() {
                let result = engine.execute_with_control(statement, &control).with_context(||format!("statement {} failed", index + 1))?;
                output(&mut writer, index + 1, result, output_limit)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> SqlArgs { SqlArgs { root: PathBuf::from("unused"), execute: None, file: None, table: None, history: false, max_scan_rows: None, max_scan_bytes: None, max_scan_work: None, max_output_bytes: None, max_history_keys: None, max_history_versions: None, max_history_bytes: None, max_staged_keys: None, max_staged_bytes: None } }

    #[test]
    fn splitting_preserves_quoted_semicolons_unicode_and_doubled_quotes() {
        let source = " ; INSERT INTO x(a) VALUES('é;two''three'); SELECT a FROM x; ;";
        assert_eq!(statements(source).unwrap(), vec!["INSERT INTO x(a) VALUES('é;two''three')", "SELECT a FROM x"]);
        assert_eq!(statements("INSERT INTO x(a) VALUES('-- /* text')").unwrap().len(), 1);
    }

    #[test]
    fn invalid_or_empty_scripts_and_statement_budgets_fail_before_execution() {
        for source in ["", " ; ", "SELECT 'open", "SELECT 'escaped''", "SELECT 1; -- comment", "/* comment */ SELECT 1"] {
            assert!(statements(source).is_err());
        }
        assert!(statements(&"x".repeat(MAX_STATEMENT)).is_ok());
        assert!(statements(&"x".repeat(MAX_STATEMENT + 1)).is_err());
        assert_eq!(statements(&"x;".repeat(MAX_STATEMENTS)).unwrap().len(), MAX_STATEMENTS);
        assert!(statements(&"x;".repeat(MAX_STATEMENTS + 1)).is_err());
    }

    #[test]
    fn input_byte_budget_and_utf8_are_checked_for_files_and_execute() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("script.sql");
        let mut input = args();
        input.file = Some(path.clone());
        std::fs::write(&path, vec![b'x'; MAX_SCRIPT]).unwrap();
        assert_eq!(read_script(&input).unwrap().len(), MAX_SCRIPT);
        std::fs::write(&path, vec![b'x'; MAX_SCRIPT + 1]).unwrap();
        assert!(read_script(&input).is_err());
        std::fs::write(&path, [255]).unwrap();
        assert!(read_script(&input).is_err());
        input.file = None;
        input.execute = Some("x".repeat(MAX_SCRIPT + 1));
        assert!(read_script(&input).is_err());
    }

    #[test]
    fn json_output_keeps_wide_numeric_and_temporal_values_exact() {
        let mut bytes = Vec::new();
        output(&mut bytes, 1, SqlResultSet::Rows { columns: vec!["id".into(),"decimal".into(),"time".into(),"uuid".into()],
            rows: vec![vec![Value::Int64(i64::MAX),Value::Decimal(10_i128.pow(38)-1),Value::TimestampTz(i64::MIN),Value::Uuid([255;16])]] }, DEFAULT_OUTPUT_BYTES).unwrap();
        let json: Json = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["rows"][0][0]["value"], i64::MAX.to_string());
        assert_eq!(json["rows"][0][1]["unscaled"], (10_i128.pow(38)-1).to_string());
        assert_eq!(json["rows"][0][2]["microseconds"], i64::MIN.to_string());
        assert_eq!(json["rows"][0][3]["value"], "ffffffff-ffff-ffff-ffff-ffffffffffff");
        assert!(bytes.ends_with(b"\n"));
    }
}
