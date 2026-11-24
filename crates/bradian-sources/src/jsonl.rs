//! JSON Lines adapter: one JSON object per line, each already shaped as a canonical
//! `bradian_ingest::Change` envelope or as a payload the caller projects into a change.
//!
//! ADR 0303 — `JsonlStream` and `JsonlEnvelopeStream`
//! process the input row-at-a-time from any `Read`.

use std::io::{BufRead, BufReader, Read};

use bradian_core::{BradianError, BradianResult};
use bradian_ingest::Change;

#[derive(Clone, Debug, Default)]
pub struct JsonlOptions {
    /// When true, every input object is interpreted as a complete `Change` envelope with
    /// the usual DHIS2-style field names (`changeId`, `siteId`, `objectType`, …). When
    /// false, the caller supplies a projection closure through `read_jsonl_with`.
    pub expect_envelope: bool,
}

pub fn read_jsonl(bytes: &[u8], options: &JsonlOptions) -> BradianResult<Vec<Change>> {
    if !options.expect_envelope {
        return Err(BradianError::invalid_value(
            "expect_envelope=false: use read_jsonl_with(options, project) instead",
        ));
    }
    let mut out = Vec::new();
    for (lineno, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let change: Change = serde_json::from_slice(line).map_err(|e| {
            BradianError::Serialization(format!("line {}: {e}", lineno + 1))
        })?;
        out.push(change);
    }
    Ok(out)
}

/// Streaming variant: call `project(value)` on each parsed JSON value and collect the
/// resulting `Change` envelopes.
pub fn read_jsonl_with<F>(bytes: &[u8], mut project: F) -> BradianResult<Vec<Change>>
where
    F: FnMut(serde_json::Value) -> BradianResult<Change>,
{
    let mut out = Vec::new();
    for (lineno, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(line).map_err(|e| {
            BradianError::Serialization(format!("line {}: {e}", lineno + 1))
        })?;
        out.push(project(value)?);
    }
    Ok(out)
}

/// ADR 0303 — streaming JSONL reader. Yields one parsed
/// `serde_json::Value` per non-blank line; parse errors
/// carry the 1-based line number.
pub struct JsonlStream<R: Read> {
    reader: BufReader<R>,
    lineno: usize,
    done:   bool,
}

impl<R: Read> JsonlStream<R> {
    pub fn new(reader: R) -> Self {
        Self { reader: BufReader::new(reader), lineno: 0, done: false }
    }
}

impl<R: Read> Iterator for JsonlStream<R> {
    type Item = BradianResult<serde_json::Value>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.done { return None; }
        let mut line = String::new();
        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => { self.done = true; return None; }
                Ok(_) => {
                    self.lineno += 1;
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() { continue; }
                    return Some(serde_json::from_str::<serde_json::Value>(trimmed)
                        .map_err(|e| BradianError::Serialization(
                            format!("line {}: {e}", self.lineno))));
                }
                Err(e) => {
                    self.done = true;
                    return Some(Err(BradianError::invalid_value(
                        format!("jsonl read: {e}"))));
                }
            }
        }
    }
}

/// ADR 0303 — streaming JSONL reader that parses each
/// record directly into a `Change` envelope.
pub struct JsonlEnvelopeStream<R: Read> { inner: JsonlStream<R> }
impl<R: Read> JsonlEnvelopeStream<R> {
    pub fn new(reader: R) -> Self { Self { inner: JsonlStream::new(reader) } }
}
impl<R: Read> Iterator for JsonlEnvelopeStream<R> {
    type Item = BradianResult<Change>;
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|r| r.and_then(|v|
            serde_json::from_value::<Change>(v).map_err(|e|
                BradianError::Serialization(format!("envelope parse: {e}")))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_envelopes() {
        let jsonl = r#"{"changeId":"c1","siteId":null,"objectType":"X","objectUid":"deAAAAAAAAA","sourceVersion":1,"operation":"UPSERT","occurredAt":"now","payload":{"a":1}}
{"changeId":"c2","siteId":null,"objectType":"X","objectUid":"deAAAAAAAAA","sourceVersion":2,"operation":"UPSERT","occurredAt":"now","payload":{"a":2}}"#;
        let changes = read_jsonl(
            jsonl.as_bytes(),
            &JsonlOptions {
                expect_envelope: true,
            },
        )
        .unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].change_id, "c1");
        assert_eq!(changes[1].source_version, 2);
    }

    // ADR 0303 — streaming parity + error reporting.

    #[test]
    fn streaming_envelope_parity_matches_batch_parser() {
        let jsonl = r#"{"changeId":"c1","siteId":null,"objectType":"X","objectUid":"deAAAAAAAAA","sourceVersion":1,"operation":"UPSERT","occurredAt":"now","payload":{"a":1}}
{"changeId":"c2","siteId":null,"objectType":"X","objectUid":"deAAAAAAAAA","sourceVersion":2,"operation":"UPSERT","occurredAt":"now","payload":{"a":2}}"#;
        let batch = read_jsonl(jsonl.as_bytes(),
            &JsonlOptions { expect_envelope: true }).unwrap();
        let stream: Vec<_> = JsonlEnvelopeStream::new(jsonl.as_bytes())
            .map(|r| r.unwrap()).collect();
        assert_eq!(stream.len(), batch.len());
        for (a, b) in stream.iter().zip(batch.iter()) {
            assert_eq!(a.change_id, b.change_id);
            assert_eq!(a.source_version, b.source_version);
        }
    }

    #[test]
    fn stream_skips_blank_lines_and_surfaces_parse_errors_with_line_number() {
        let data = "{\"ok\":1}\n\n{not-json}\n{\"ok\":2}\n";
        let mut s = JsonlStream::new(data.as_bytes());
        assert_eq!(s.next().unwrap().unwrap()["ok"], 1);
        // Blank line skipped; next call hits the malformed
        // record and surfaces the 1-based line number 3.
        let err = s.next().unwrap().unwrap_err();
        assert!(format!("{err}").contains("line 3"),
            "expected line-3 error context, got: {err}");
        // After the error, the stream remains usable for
        // subsequent lines.
        assert_eq!(s.next().unwrap().unwrap()["ok"], 2);
    }

    #[test]
    fn with_projection_derives_envelope() {
        let jsonl = r#"{"id":1,"amount":10}
{"id":2,"amount":20}"#;
        let changes = read_jsonl_with(jsonl.as_bytes(), |v| {
            let id = v["id"].as_i64().unwrap();
            Ok(Change {
                change_id: format!("row-{id}"),
                site_id: None,
                object_type: "SALE".into(),
                object_uid: bradian_core::Uid::new("saleXXXXXXX").unwrap(),
                source_version: id as u64,
                operation: bradian_ingest::Operation::Upsert,
                occurred_at: "now".into(),
                metadata_revision: None,
                payload_hash: None,
                payload: v,
            })
        })
        .unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[1].change_id, "row-2");
    }
}
