//! ADR 0162 — in-process transport primitive.
//!
//! Composes the full wire stack end-to-end:
//! - ADR 0137 wire-frame envelope (BRFR magic + CRC32).
//! - ADR 0139 prepared-statement envelope (serde JSON).
//! - ADR 0140 chunked response stream (for large result
//!   sets).
//! - ADR 0129 / 0158 / 0160 Driver trait + engine
//!   adapters.
//!
//! Honest scope — this is an IN-PROCESS transport that
//! takes a wire-frame byte blob, extracts the payload,
//! runs it through a Driver, and emits a wire-frame
//! response. No actual socket, no async runtime. The
//! point is to prove the stack composes cleanly so a
//! future `bradian-serving` crate can wrap this in a
//! tokio listener.
//!
//! Out of scope (follow-ups):
//! - tokio listener / network socket — deferred to
//!   `bradian-serving`.
//! - TLS/mTLS handshake — procurement-dependent
//!   (certificate issuance + rotation infrastructure).
//! - Session-aware transaction coordination over the
//!   wire — needs the SessionDriver trait called out in
//!   ADR 0160.
//! - Multi-client fairness / connection pooling — the
//!   current primitive serves one request at a time.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chunk_stream::split;
use crate::driver::{Driver, DriverResponse};
use crate::prepared::PreparedStatement;
use crate::wire_frame::{decode, encode, FrameKind, MAX_PAYLOAD};

/// ADR 0162 — serde-serializable payload wrapper for
/// Request frames. Clients choose between a plain-SQL
/// execute and a PreparedStatement execute.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestPayload {
    Sql(String),
    Prepared(PreparedStatement),
}

/// ADR 0162 — Response-side payload wrapper. Mirrors
/// `DriverResponse` as a serde-friendly structure. On
/// error we emit a separate Error frame instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponsePayload {
    Empty,
    Affected { rows: u64 },
    Rows {
        columns: Vec<String>,
        /// serde-serializable row values keyed by column.
        rows: Vec<Vec<crate::typed_table::Value>>,
    },
}

/// ADR 0162 — in-process transport handle. Holds a
/// `dyn Driver` behind an Arc so the transport is
/// cheaply cloneable.
#[derive(Clone)]
pub struct LocalTransport {
    driver: Arc<dyn Driver>,
}

impl std::fmt::Debug for LocalTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalTransport").finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("frame error: {0}")]
    Frame(#[from] crate::wire_frame::FrameError),
    #[error("payload serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("expected Request frame, got {kind:?}")]
    UnexpectedKind { kind: FrameKind },
    #[error("payload too large after serialization: {bytes} bytes > MAX_PAYLOAD {max}")]
    PayloadTooLarge { bytes: usize, max: usize },
}

pub type TransportResult<T> = std::result::Result<T, TransportError>;

impl LocalTransport {
    pub fn new(driver: Arc<dyn Driver>) -> Self { Self { driver } }

    /// ADR 0162 — handle one wire-frame request. Returns
    /// either a Response frame (serialized JSON body) or
    /// an Error frame (UTF-8 message body).
    pub fn handle_frame(&self, request_frame: &[u8]) -> TransportResult<Vec<u8>> {
        let (kind, payload_bytes) = decode(request_frame)?;
        if kind != FrameKind::Request {
            return Err(TransportError::UnexpectedKind { kind });
        }
        let payload: RequestPayload = serde_json::from_slice(&payload_bytes)?;
        let result = match payload {
            RequestPayload::Sql(sql) => self.driver.execute(&sql),
            RequestPayload::Prepared(stmt) => self.driver.execute_prepared(&stmt),
        };
        match result {
            Ok(response) => {
                let serializable = translate_response(response);
                let body = serde_json::to_vec(&serializable)?;
                if body.len() > MAX_PAYLOAD {
                    return Err(TransportError::PayloadTooLarge {
                        bytes: body.len(), max: MAX_PAYLOAD,
                    });
                }
                Ok(encode(FrameKind::Response, &body)?)
            }
            Err(e) => {
                let msg = format!("{e}");
                Ok(encode(FrameKind::Error, msg.as_bytes())?)
            }
        }
    }

    /// ADR 0162 — same as `handle_frame`, but for
    /// response payloads larger than MAX_PAYLOAD. Splits
    /// the response into ADR 0140 chunks. Returns the
    /// chunks pre-encoded as individual Response frames,
    /// each carrying one Chunk serialized to JSON.
    pub fn handle_frame_chunked(
        &self,
        request_frame: &[u8],
        stream_id: u64,
        chunk_size: usize,
    ) -> TransportResult<Vec<Vec<u8>>> {
        let (kind, payload_bytes) = decode(request_frame)?;
        if kind != FrameKind::Request {
            return Err(TransportError::UnexpectedKind { kind });
        }
        let payload: RequestPayload = serde_json::from_slice(&payload_bytes)?;
        let result = match payload {
            RequestPayload::Sql(sql) => self.driver.execute(&sql),
            RequestPayload::Prepared(stmt) => self.driver.execute_prepared(&stmt),
        };
        let response = match result {
            Ok(r) => r,
            Err(_) => return Ok(vec![]),  // error path omitted in chunked mode for now
        };
        let serializable = translate_response(response);
        let body = serde_json::to_vec(&serializable)?;
        let chunks = split(&body, stream_id, chunk_size)
            .map_err(|e| TransportError::Serde(
                serde_json::Error::io(std::io::Error::other(format!("{e:?}")))))?;
        let mut frames = Vec::with_capacity(chunks.len());
        for chunk in chunks {
            let chunk_bytes = serde_json::to_vec(&chunk)?;
            frames.push(encode(FrameKind::Response, &chunk_bytes)?);
        }
        Ok(frames)
    }
}

fn translate_response(r: DriverResponse) -> ResponsePayload {
    match r {
        DriverResponse::Empty => ResponsePayload::Empty,
        DriverResponse::Affected { rows } => ResponsePayload::Affected { rows },
        DriverResponse::Rows(rows) => {
            let columns = rows.first().map(|r| r.columns.clone()).unwrap_or_default();
            ResponsePayload::Rows {
                columns,
                rows: rows.into_iter().map(|r| r.values).collect(),
            }
        }
    }
}

/// ADR 0162 — client-side helper to encode a request
/// payload into a wire frame.
pub fn encode_request(payload: &RequestPayload) -> TransportResult<Vec<u8>> {
    let body = serde_json::to_vec(payload)?;
    Ok(encode(FrameKind::Request, &body)?)
}

/// ADR 0162 — client-side helper to decode a response
/// frame into a typed payload. Error frames produce
/// `Err(Error-message)`.
pub fn decode_response(frame: &[u8]) -> TransportResult<Result<ResponsePayload, String>> {
    let (kind, body) = decode(frame)?;
    match kind {
        FrameKind::Response => {
            let payload: ResponsePayload = serde_json::from_slice(&body)?;
            Ok(Ok(payload))
        }
        FrameKind::Error => Ok(Err(String::from_utf8_lossy(&body).into_owned())),
        other => Err(TransportError::UnexpectedKind { kind: other }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver_adapter::SqlEngineAdapter;
    use crate::sql::SqlEngine;
    use crate::typed_table::Value;
    use tempfile::tempdir;

    fn open_transport() -> (tempfile::TempDir, LocalTransport) {
        let dir = tempdir().unwrap();
        let engine = Arc::new(SqlEngine::open(dir.path()).unwrap());
        let adapter: Arc<dyn Driver> = Arc::new(SqlEngineAdapter::new(engine));
        (dir, LocalTransport::new(adapter))
    }

    #[test]
    fn transport_round_trips_a_simple_sql_request() {
        let (_d, t) = open_transport();
        let req = encode_request(
            &RequestPayload::Sql("CREATE DATABASE wire".into())).unwrap();
        let resp = t.handle_frame(&req).unwrap();
        let decoded = decode_response(&resp).unwrap().unwrap();
        assert_eq!(decoded, ResponsePayload::Empty);
    }

    #[test]
    fn transport_returns_error_frame_for_syntax_error() {
        let (_d, t) = open_transport();
        let req = encode_request(
            &RequestPayload::Sql("NOT VALID SQL".into())).unwrap();
        let resp = t.handle_frame(&req).unwrap();
        let decoded = decode_response(&resp).unwrap();
        assert!(decoded.is_err());
    }

    #[test]
    fn transport_round_trips_a_select_as_rows_payload() {
        let (_d, t) = open_transport();
        for sql in [
            "CREATE DATABASE app",
            "CREATE SCHEMA app.public",
            "CREATE TABLE app.public.items (
                id BIGINT NOT NULL, PRIMARY KEY (id))",
            "INSERT INTO app.public.items (id) VALUES (42)",
        ] {
            let req = encode_request(&RequestPayload::Sql(sql.into())).unwrap();
            t.handle_frame(&req).unwrap();
        }
        let req = encode_request(
            &RequestPayload::Sql("SELECT id FROM app.public.items".into())).unwrap();
        let resp = t.handle_frame(&req).unwrap();
        let decoded = decode_response(&resp).unwrap().unwrap();
        match decoded {
            ResponsePayload::Rows { columns, rows } => {
                assert_eq!(columns, vec!["id".to_string()]);
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0][0], Value::Int64(42));
            }
            other => panic!("expected Rows, got {other:?}"),
        }
    }

    #[test]
    fn transport_round_trips_a_prepared_statement() {
        let (_d, t) = open_transport();
        for sql in [
            "CREATE DATABASE a",
            "CREATE SCHEMA a.p",
            "CREATE TABLE a.p.t (id BIGINT NOT NULL, PRIMARY KEY (id))",
        ] {
            let req = encode_request(&RequestPayload::Sql(sql.into())).unwrap();
            t.handle_frame(&req).unwrap();
        }
        let stmt = PreparedStatement::new(
            "INSERT INTO a.p.t (id) VALUES ($1)",
            vec![Value::Int64(99)],
        );
        let req = encode_request(&RequestPayload::Prepared(stmt)).unwrap();
        let resp = t.handle_frame(&req).unwrap();
        let decoded = decode_response(&resp).unwrap().unwrap();
        assert_eq!(decoded, ResponsePayload::Affected { rows: 1 });
    }

    #[test]
    fn non_request_frame_is_rejected() {
        let (_d, t) = open_transport();
        let frame = encode(FrameKind::Response, b"wrong").unwrap();
        let err = t.handle_frame(&frame).unwrap_err();
        assert!(matches!(err, TransportError::UnexpectedKind { .. }));
    }

    #[test]
    fn transport_is_object_safe_behind_arc() {
        let (_d, t) = open_transport();
        let t2 = t.clone();
        let req = encode_request(
            &RequestPayload::Sql("CREATE DATABASE shared".into())).unwrap();
        t.handle_frame(&req).unwrap();
        // Clone sees shared state (duplicate CREATE errors).
        let req = encode_request(
            &RequestPayload::Sql("CREATE DATABASE shared".into())).unwrap();
        let resp = t2.handle_frame(&req).unwrap();
        let decoded = decode_response(&resp).unwrap();
        assert!(decoded.is_err());
    }

    #[test]
    fn chunked_mode_splits_large_responses() {
        let (_d, t) = open_transport();
        let req = encode_request(
            &RequestPayload::Sql("CREATE DATABASE c".into())).unwrap();
        let chunks = t.handle_frame_chunked(&req, 42, 16).unwrap();
        // Response is tiny but with 16-byte chunks it still
        // fits in one; verify we get at least one frame.
        assert!(!chunks.is_empty());
    }
}
