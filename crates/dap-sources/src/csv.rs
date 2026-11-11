//! CSV adapter.
//!
//! Minimal RFC 4180-ish parser: comma separator by default, double-quoted fields with "" as
//! an embedded quote, CRLF or LF line endings. The parser hands rows to a caller-supplied
//! closure that turns them into `dap_ingest::Change` envelopes.
//!
//! The reader is pure Rust (no `csv` crate dependency) so the audit surface stays small.
//! Downstream callers who need sharded parallel ingestion can wrap this iterator in rayon.

use dap_core::{DapError, DapResult};
use dap_ingest::Change;

#[derive(Clone, Debug)]
pub struct CsvOptions {
    pub delimiter: u8,
    pub quote: u8,
    pub has_header: bool,
}

impl Default for CsvOptions {
    fn default() -> Self {
        Self {
            delimiter: b',',
            quote: b'"',
            has_header: true,
        }
    }
}

pub type CsvRow = Vec<String>;
pub type CsvHeader = Option<Vec<String>>;

/// Parse CSV bytes into a vector of rows. The first row is returned separately as the
/// header when `options.has_header` is true. Fields are already unescaped.
pub fn read_csv_bytes(
    bytes: &[u8],
    options: &CsvOptions,
) -> DapResult<(CsvHeader, Vec<CsvRow>)> {
    let mut rows = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_field = String::new();
    let mut in_quote = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if in_quote {
            if b == options.quote {
                // Doubled quote inside a quoted field is an escaped quote.
                if bytes.get(i + 1) == Some(&options.quote) {
                    current_field.push(options.quote as char);
                    i += 2;
                    continue;
                }
                in_quote = false;
                i += 1;
                continue;
            }
            current_field.push(b as char);
            i += 1;
            continue;
        }
        match b {
            x if x == options.quote => {
                in_quote = true;
                i += 1;
            }
            x if x == options.delimiter => {
                current_row.push(std::mem::take(&mut current_field));
                i += 1;
            }
            b'\r' => {
                // Treat CR as part of a line terminator. Skip and let the LF path pick it up.
                i += 1;
            }
            b'\n' => {
                current_row.push(std::mem::take(&mut current_field));
                rows.push(std::mem::take(&mut current_row));
                i += 1;
            }
            _ => {
                current_field.push(b as char);
                i += 1;
            }
        }
    }
    if in_quote {
        return Err(DapError::invalid_value("unterminated CSV quoted field"));
    }
    // Capture the last field + row if the file does not end with a newline.
    if !current_field.is_empty() || !current_row.is_empty() {
        current_row.push(current_field);
        rows.push(current_row);
    }

    let header = if options.has_header && !rows.is_empty() {
        Some(rows.remove(0))
    } else {
        None
    };
    Ok((header, rows))
}

/// Shape helper: turn each CSV row into a canonical `Change` envelope by mapping column
/// names to payload keys. The closure must produce the `object_type`, `object_uid`,
/// `source_version` and `operation` for the row; the payload defaults to a JSON object
/// containing every column.
type JsonMap = serde_json::Map<String, serde_json::Value>;

pub fn rows_to_changes<F>(
    header: &[String],
    rows: Vec<Vec<String>>,
    mut project: F,
) -> DapResult<Vec<Change>>
where
    F: FnMut(&JsonMap) -> DapResult<ChangeShell>,
{
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.len() != header.len() {
            return Err(DapError::invalid_value(format!(
                "CSV row has {} fields, header has {}",
                row.len(),
                header.len()
            )));
        }
        let mut payload = serde_json::Map::new();
        for (name, value) in header.iter().zip(row.into_iter()) {
            payload.insert(name.clone(), serde_json::Value::String(value));
        }
        let shell = project(&payload)?;
        out.push(Change {
            change_id: shell.change_id,
            site_id: shell.site_id,
            object_type: shell.object_type,
            object_uid: shell.object_uid,
            source_version: shell.source_version,
            operation: shell.operation,
            occurred_at: shell.occurred_at,
            metadata_revision: None,
            payload_hash: None,
            payload: serde_json::Value::Object(payload),
        });
    }
    Ok(out)
}

/// Light shell carrying the envelope header fields the projection function must supply;
/// keeps `rows_to_changes` ergonomic while still letting the caller choose how to derive
/// each stamp from the CSV columns.
#[derive(Clone, Debug)]
pub struct ChangeShell {
    pub change_id: String,
    pub site_id: Option<String>,
    pub object_type: String,
    pub object_uid: dap_core::Uid,
    pub source_version: u64,
    pub operation: dap_ingest::Operation,
    pub occurred_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_csv_with_header() {
        let data = "ou,period,value\nouA,202601,10\nouB,202601,20\n";
        let (header, rows) = read_csv_bytes(data.as_bytes(), &CsvOptions::default()).unwrap();
        assert_eq!(header.unwrap(), vec!["ou", "period", "value"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], vec!["ouA", "202601", "10"]);
    }

    #[test]
    fn handles_quoted_fields_with_embedded_commas() {
        let data = "name,note\n\"Bob, Jr.\",\"line with, comma\"\n";
        let (_, rows) = read_csv_bytes(data.as_bytes(), &CsvOptions::default()).unwrap();
        assert_eq!(rows[0], vec!["Bob, Jr.", "line with, comma"]);
    }

    #[test]
    fn handles_escaped_double_quote() {
        let data = "field\n\"He said \"\"hello\"\"\"\n";
        let (_, rows) = read_csv_bytes(data.as_bytes(), &CsvOptions::default()).unwrap();
        assert_eq!(rows[0], vec!["He said \"hello\""]);
    }

    #[test]
    fn unterminated_quote_rejected() {
        let data = "a\n\"unterminated\n";
        assert!(read_csv_bytes(data.as_bytes(), &CsvOptions::default()).is_err());
    }

    #[test]
    fn rows_to_changes_projection() {
        let (header, rows) = read_csv_bytes(
            "ou,period,value\nouAAAAAAAAA,202601,10\n".as_bytes(),
            &CsvOptions::default(),
        )
        .unwrap();
        let header = header.unwrap();
        let changes = rows_to_changes(&header, rows, |row| {
            let ou = row.get("ou").and_then(|v| v.as_str()).unwrap_or("");
            Ok(ChangeShell {
                change_id: format!("csv-{ou}"),
                site_id: None,
                object_type: "AGGREGATE_DATA_VALUE".into(),
                object_uid: dap_core::Uid::new("deAAAAAAAAA").unwrap(),
                source_version: 1,
                operation: dap_ingest::Operation::Upsert,
                occurred_at: "now".into(),
            })
        })
        .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_id, "csv-ouAAAAAAAAA");
    }
}
