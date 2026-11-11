//! JSON Lines adapter: one JSON object per line, each already shaped as a canonical
//! `dap_ingest::Change` envelope or as a payload the caller projects into a change.

use dap_core::{DapError, DapResult};
use dap_ingest::Change;

#[derive(Clone, Debug, Default)]
pub struct JsonlOptions {
    /// When true, every input object is interpreted as a complete `Change` envelope with
    /// the usual DHIS2-style field names (`changeId`, `siteId`, `objectType`, …). When
    /// false, the caller supplies a projection closure through `read_jsonl_with`.
    pub expect_envelope: bool,
}

pub fn read_jsonl(bytes: &[u8], options: &JsonlOptions) -> DapResult<Vec<Change>> {
    if !options.expect_envelope {
        return Err(DapError::invalid_value(
            "expect_envelope=false: use read_jsonl_with(options, project) instead",
        ));
    }
    let mut out = Vec::new();
    for (lineno, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let change: Change = serde_json::from_slice(line).map_err(|e| {
            DapError::Serialization(format!("line {}: {e}", lineno + 1))
        })?;
        out.push(change);
    }
    Ok(out)
}

/// Streaming variant: call `project(value)` on each parsed JSON value and collect the
/// resulting `Change` envelopes.
pub fn read_jsonl_with<F>(bytes: &[u8], mut project: F) -> DapResult<Vec<Change>>
where
    F: FnMut(serde_json::Value) -> DapResult<Change>,
{
    let mut out = Vec::new();
    for (lineno, line) in bytes.split(|b| *b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(line).map_err(|e| {
            DapError::Serialization(format!("line {}: {e}", lineno + 1))
        })?;
        out.push(project(value)?);
    }
    Ok(out)
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
                object_uid: dap_core::Uid::new("saleXXXXXXX").unwrap(),
                source_version: id as u64,
                operation: dap_ingest::Operation::Upsert,
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
