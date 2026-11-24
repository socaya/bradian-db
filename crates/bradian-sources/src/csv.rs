//! CSV adapter.
//!
//! Minimal RFC 4180-ish parser: comma separator by default, double-quoted fields with "" as
//! an embedded quote, CRLF or LF line endings. The parser hands rows to a caller-supplied
//! closure that turns them into `bradian_ingest::Change` envelopes.
//!
//! The reader is pure Rust (no `csv` crate dependency) so the audit surface stays small.
//! Downstream callers who need sharded parallel ingestion can wrap this iterator in rayon.

use std::io::{BufRead, BufReader, Read};

use bradian_core::{BradianError, BradianResult};
use bradian_ingest::Change;

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
) -> BradianResult<(CsvHeader, Vec<CsvRow>)> {
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
        return Err(BradianError::invalid_value("unterminated CSV quoted field"));
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
) -> BradianResult<Vec<Change>>
where
    F: FnMut(&JsonMap) -> BradianResult<ChangeShell>,
{
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        if row.len() != header.len() {
            return Err(BradianError::invalid_value(format!(
                "CSV row has {} fields, header has {}",
                row.len(),
                header.len()
            )));
        }
        let mut payload = serde_json::Map::new();
        for (name, value) in header.iter().zip(row) {
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

/// ADR 0302 — streaming CSV reader. Parses one row at a
/// time from any `Read`; holds only the current row in
/// memory (plus a small line buffer). A `Some(header)`
/// is consumed before the first `next()` returns a data
/// row.
pub struct CsvStream<R: Read> {
    reader:  BufReader<R>,
    options: CsvOptions,
    header:  Option<Vec<String>>,
    header_consumed: bool,
    done:    bool,
    /// Line buffer; reused across rows.
    line:    String,
}

impl<R: Read> CsvStream<R> {
    pub fn new(reader: R, options: CsvOptions) -> Self {
        Self {
            reader: BufReader::new(reader),
            options, header: None, header_consumed: false,
            done: false, line: String::new(),
        }
    }

    pub fn header(&mut self) -> BradianResult<Option<&[String]>> {
        if !self.header_consumed && self.options.has_header {
            self.consume_header()?;
        }
        Ok(self.header.as_deref())
    }

    fn consume_header(&mut self) -> BradianResult<()> {
        self.header_consumed = true;
        if let Some(row) = self.read_next_logical_row()? {
            self.header = Some(row);
        }
        Ok(())
    }

    /// Read one logical row, honouring multi-line quoted
    /// fields. Returns `None` at EOF.
    fn read_next_logical_row(&mut self) -> BradianResult<Option<Vec<String>>> {
        if self.done { return Ok(None); }
        let mut accumulated = String::new();
        loop {
            self.line.clear();
            let n = self.reader.read_line(&mut self.line).map_err(|e|
                BradianError::invalid_value(format!("csv read: {e}")))?;
            if n == 0 {
                self.done = true;
                if accumulated.is_empty() { return Ok(None); }
                break;
            }
            accumulated.push_str(&self.line);
            if is_balanced(&accumulated, self.options.quote as char) { break; }
        }
        // Reuse the batch parser on this single row (it's
        // tiny; no need for a second state machine). Force
        // no-header so the parser doesn't eat our one row.
        let mut opts = self.options.clone();
        opts.has_header = false;
        let (_hdr, mut rows) = read_csv_bytes(accumulated.as_bytes(), &opts)?;
        if rows.is_empty() { return Ok(None); }
        Ok(Some(rows.remove(0)))
    }
}

/// True when every opening quote in the buffer is balanced
/// by a closing quote (doubled quotes inside quoted fields
/// are escapes and count as two separators).
fn is_balanced(buf: &str, quote: char) -> bool {
    let mut in_q = false;
    let mut chars = buf.chars().peekable();
    while let Some(c) = chars.next() {
        if c == quote {
            if in_q && chars.peek() == Some(&quote) { chars.next(); continue; }
            in_q = !in_q;
        }
    }
    !in_q
}

impl<R: Read> Iterator for CsvStream<R> {
    type Item = BradianResult<CsvRow>;
    fn next(&mut self) -> Option<Self::Item> {
        if !self.header_consumed && self.options.has_header {
            if let Err(e) = self.consume_header() { return Some(Err(e)); }
        }
        match self.read_next_logical_row() {
            Ok(Some(row)) => Some(Ok(row)),
            Ok(None)      => None,
            Err(e)        => Some(Err(e)),
        }
    }
}

/// ADR 0302 — inferred column type from a sample of CSV
/// rows. Narrow → wide: `Bool ⊂ I64 ⊂ F64 ⊂ Text`. An
/// all-empty column is `Text`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferredType { Bool, I64, F64, Text }

pub fn infer_column_types(header: &[String], rows: &[CsvRow]) -> Vec<InferredType> {
    let mut out = vec![None::<InferredType>; header.len()];
    for row in rows {
        for (i, field) in row.iter().enumerate() {
            if i >= out.len() { break; }
            if field.is_empty() { continue; }
            let observed = classify_one(field);
            out[i] = Some(match out[i] {
                None       => observed,
                Some(prev) => widen(prev, observed),
            });
        }
    }
    out.into_iter().map(|t| t.unwrap_or(InferredType::Text)).collect()
}

fn classify_one(field: &str) -> InferredType {
    let lower = field.to_ascii_lowercase();
    if lower == "true" || lower == "false" { return InferredType::Bool; }
    if field.parse::<i64>().is_ok() { return InferredType::I64; }
    if field.parse::<f64>().is_ok() { return InferredType::F64; }
    InferredType::Text
}

fn widen(a: InferredType, b: InferredType) -> InferredType {
    use InferredType::*;
    fn rank(t: InferredType) -> u8 {
        match t { Bool => 0, I64 => 1, F64 => 2, Text => 3 }
    }
    if rank(a) >= rank(b) { a } else { b }
}

/// Light shell carrying the envelope header fields the projection function must supply;
/// keeps `rows_to_changes` ergonomic while still letting the caller choose how to derive
/// each stamp from the CSV columns.
#[derive(Clone, Debug)]
pub struct ChangeShell {
    pub change_id: String,
    pub site_id: Option<String>,
    pub object_type: String,
    pub object_uid: bradian_core::Uid,
    pub source_version: u64,
    pub operation: bradian_ingest::Operation,
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

    // ADR 0302 — streaming + type inference.

    #[test]
    fn streaming_parse_matches_batch_parse_on_three_row_input() {
        let data = "ou,period,value\nouA,202601,10\nouB,202601,20\nouC,202601,30\n";
        let (hdr_batch, rows_batch) = read_csv_bytes(data.as_bytes(), &CsvOptions::default()).unwrap();
        let mut stream = CsvStream::new(data.as_bytes(), CsvOptions::default());
        let hdr_stream = stream.header().unwrap().map(|s| s.to_vec());
        let rows_stream: Vec<_> = stream.map(|r| r.unwrap()).collect();
        assert_eq!(hdr_stream, hdr_batch);
        assert_eq!(rows_stream, rows_batch);
    }

    #[test]
    fn type_inference_classifies_each_column_narrowest_first() {
        let data = "id,amount,ratio,active,label\n1,100,0.5,true,alpha\n2,200,1.25,false,beta\n3,300,2.0,true,gamma\n";
        let (hdr, rows) = read_csv_bytes(data.as_bytes(), &CsvOptions::default()).unwrap();
        let hdr = hdr.unwrap();
        let types = infer_column_types(&hdr, &rows);
        assert_eq!(types, vec![
            InferredType::I64,
            InferredType::I64,
            InferredType::F64,
            InferredType::Bool,
            InferredType::Text,
        ]);
    }

    #[test]
    fn type_inference_widens_on_conflict_and_defaults_empty_to_text() {
        let data = "mixed,empty,int_then_float\n1,,1\n2,,1.5\n3,,2\n";
        let (hdr, rows) = read_csv_bytes(data.as_bytes(), &CsvOptions::default()).unwrap();
        let hdr = hdr.unwrap();
        let types = infer_column_types(&hdr, &rows);
        assert_eq!(types[0], InferredType::I64);
        assert_eq!(types[1], InferredType::Text,
            "all-empty column should default to Text");
        assert_eq!(types[2], InferredType::F64,
            "I64 + F64 should widen to F64");
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
                object_uid: bradian_core::Uid::new("deAAAAAAAAA").unwrap(),
                source_version: 1,
                operation: bradian_ingest::Operation::Upsert,
                occurred_at: "now".into(),
            })
        })
        .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].change_id, "csv-ouAAAAAAAAA");
    }
}
