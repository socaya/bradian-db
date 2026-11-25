//! ADR 0316 — Relational wire protocol + reference driver
//! (plaintext TCP).
//!
//! ADR 0330 — TLS-wrapped variant of the same protocol
//! using `tokio-rustls`. The framing + handshake logic
//! is generic over `AsyncRead + AsyncWrite + Unpin`, so
//! the server + client handle the plaintext and TLS
//! streams through one implementation.

use std::io;
use std::sync::Arc;

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{rustls::pki_types::ServerName, TlsAcceptor, TlsConnector};

/// Max frame size (1 MiB). Refuse anything larger so a
/// buggy / hostile peer cannot allocate unbounded memory
/// on the far side.
pub const MAX_FRAME_BYTES: u32 = 1024 * 1024;

/// ADR 0338 — typed wire value. Mirrors
/// `bradian_relational::typed_table::Value` so clients
/// can parse row cells back into language-level types
/// without a debug-format round-trip.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireValue {
    Null,
    Bool { value: bool },
    Int32 { value: i32 },
    Int64 { value: i64 },
    /// Signed 128-bit decimal shipped as a decimal
    /// string. Column scale is published in the RowSet
    /// header's `column_types` list (ADR 0340).
    Decimal { value: String },
    String { value: String },
    /// Microseconds since Unix epoch (UTC).
    TimestampTz { micros: i64 },
    /// Hex-encoded 16-byte UUID (canonical dashed form).
    Uuid { value: String },
    Json { value: String },
}

/// ADR 0340 — per-column wire type hint. The
/// `WireMessage::RowSet.column_types` list carries one
/// entry per column so a client knows the expected cell
/// variant before parsing the first row — essential for
/// empty-row-set responses where variant inference from
/// row data is impossible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireType {
    /// Column type could not be inferred (empty row set,
    /// or engine returned no type metadata). Clients
    /// SHOULD treat this as "accept any variant".
    Unknown,
    Bool,
    Int32,
    Int64,
    Decimal,
    String,
    TimestampTz,
    Uuid,
    Json,
}

impl WireValue {
    /// Return the `WireType` discriminant of this value.
    /// `Null` maps to `Unknown` since a single null tells
    /// a client nothing about the column's declared type.
    pub fn wire_type(&self) -> WireType {
        match self {
            WireValue::Null              => WireType::Unknown,
            WireValue::Bool { .. }       => WireType::Bool,
            WireValue::Int32 { .. }      => WireType::Int32,
            WireValue::Int64 { .. }      => WireType::Int64,
            WireValue::Decimal { .. }    => WireType::Decimal,
            WireValue::String { .. }     => WireType::String,
            WireValue::TimestampTz { .. }=> WireType::TimestampTz,
            WireValue::Uuid { .. }       => WireType::Uuid,
            WireValue::Json { .. }       => WireType::Json,
        }
    }
}

/// Infer per-column types from the row set's data:
/// scan the first non-null cell per column; columns
/// that remain all-null (or empty row set) stay
/// `Unknown`. This is a best-effort shape until the
/// engine ships column metadata in `SqlResultSet`.
fn infer_column_types(columns: &[String], rows: &[Vec<WireValue>]) -> Vec<WireType> {
    let mut out = vec![WireType::Unknown; columns.len()];
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(out.len()) {
            if matches!(out[i], WireType::Unknown) {
                let t = cell.wire_type();
                if !matches!(t, WireType::Unknown) { out[i] = t; }
            }
        }
        if out.iter().all(|t| !matches!(t, WireType::Unknown)) { break; }
    }
    out
}

impl From<bradian_relational::typed_table::Value> for WireValue {
    fn from(v: bradian_relational::typed_table::Value) -> Self {
        use bradian_relational::typed_table::Value as R;
        match v {
            R::Null              => WireValue::Null,
            R::Boolean(b)        => WireValue::Bool  { value: b },
            R::Int32(i)          => WireValue::Int32 { value: i },
            R::Int64(i)          => WireValue::Int64 { value: i },
            R::Decimal(d)        => WireValue::Decimal { value: d.to_string() },
            R::String(s)         => WireValue::String  { value: s },
            R::TimestampTz(t)    => WireValue::TimestampTz { micros: t },
            R::Uuid(b)           => WireValue::Uuid { value: format_uuid(&b) },
            R::Json(s)           => WireValue::Json { value: s },
        }
    }
}

fn format_uuid(b: &[u8; 16]) -> String {
    // Canonical 8-4-4-4-12 dashed form.
    let mut out = String::with_capacity(36);
    for (i, byte) in b.iter().enumerate() {
        if i == 4 || i == 6 || i == 8 || i == 10 { out.push('-'); }
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Cheap byte-cost estimator for the memory-pressure
/// gate. Owned byte-buffers only; constant overhead is
/// ignored since it is dominated by payload size for
/// non-trivial result sets.
fn wire_value_bytes(v: &WireValue) -> usize {
    match v {
        WireValue::Null                  => 1,
        WireValue::Bool { .. }           => 1,
        WireValue::Int32 { .. }          => 4,
        WireValue::Int64 { .. }          => 8,
        WireValue::Decimal { value }     => value.len(),
        WireValue::String  { value }     => value.len(),
        WireValue::TimestampTz { .. }    => 8,
        WireValue::Uuid { value }        => value.len(),
        WireValue::Json { value }        => value.len(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Hello    { version: u32, principal: String },
    HelloOk  { session: u64 },
    Execute  { sql: String },
    /// ADR 0340 — `column_types` carries one `WireType`
    /// per entry in `columns`, best-effort inferred from
    /// the row data (or `Unknown` on empty / all-null).
    RowSet   {
        columns: Vec<String>,
        column_types: Vec<WireType>,
        rows: Vec<Vec<WireValue>>,
    },
    Command  { affected: u64 },
    ErrorMsg { code: u32, message: String },
    Bye,
}

#[derive(Debug, Error)]
pub enum WireError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("handshake mismatch: {0}")]
    Handshake(String),
    #[error("frame exceeds MAX_FRAME_BYTES ({actual} > {max})")]
    FrameTooLarge { actual: u32, max: u32 },
    #[error("unexpected message: {0:?}")]
    Unexpected(WireMessage),
    #[error("peer closed the connection")]
    Closed,
    #[error("tls: {0}")]
    Tls(String),
    #[error("auth: {0}")]
    Auth(String),
}
pub type WireResult<T> = Result<T, WireError>;

/// Framed write: 4-byte big-endian length + JSON body.
async fn write_frame<W: AsyncWriteExt + Unpin>(
    w: &mut W, msg: &WireMessage,
) -> WireResult<()> {
    let body = serde_json::to_vec(msg)?;
    if body.len() as u32 > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge {
            actual: body.len() as u32, max: MAX_FRAME_BYTES });
    }
    w.write_all(&(body.len() as u32).to_be_bytes()).await?;
    w.write_all(&body).await?;
    w.flush().await?;
    Ok(())
}

/// Framed read: 4-byte big-endian length + JSON body.
/// Returns `WireError::Closed` on orderly peer FIN.
async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> WireResult<WireMessage> {
    let mut len_bytes = [0u8; 4];
    match r.read_exact(&mut len_bytes).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Err(WireError::Closed),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_bytes);
    if len > MAX_FRAME_BYTES {
        return Err(WireError::FrameTooLarge { actual: len, max: MAX_FRAME_BYTES });
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}

// ---------------------------------------------------------------------------
// Server

/// Wire-protocol server bound to a real `SqlEngine`.
#[derive(Clone)]
pub struct WireServer {
    engine: Arc<SqlEngine>,
    next_session: Arc<std::sync::atomic::AtomicU64>,
    /// ADR 0337 — optional memory-pressure policy consulted
    /// on every `RowSet` emission. `None` → no admission
    /// (preserves pre-0337 behaviour).
    memory_pressure:
        Option<Arc<bradian_relational::memory_pressure::MemoryPressureHarness>>,
    /// ADR 0339 — optional audit log. When set, every
    /// authenticated statement writes a
    /// `bradian_security::AuditEntry` record.
    audit: Option<Arc<bradian_security::AuditLog>>,
}

impl std::fmt::Debug for WireServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireServer").finish()
    }
}

impl WireServer {
    pub fn new(engine: Arc<SqlEngine>) -> Self {
        Self {
            engine,
            next_session: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            memory_pressure: None,
            audit: None,
        }
    }

    /// ADR 0337 — install a `MemoryPressureHarness` that
    /// gates every `RowSet` emission. Before formatting
    /// rows into a wire frame, the handler admits the
    /// estimated byte size of the result set against the
    /// harness; a refusal is reported as `ErrorMsg { code:
    /// 4 }` without executing the frame.
    pub fn with_memory_pressure(
        mut self,
        harness: Arc<bradian_relational::memory_pressure::MemoryPressureHarness>,
    ) -> Self {
        self.memory_pressure = Some(harness);
        self
    }

    /// ADR 0339 — install a shared `AuditLog`. When set,
    /// the wire handler appends an `AuditEntry` for every
    /// authenticated statement (success, authz denial,
    /// pressure denial, and engine error) plus one
    /// connection-close entry.
    pub fn with_audit_log(mut self, log: Arc<bradian_security::AuditLog>) -> Self {
        self.audit = Some(log);
        self
    }

    /// Accept one connection from `listener`, drive the
    /// handshake, then loop reading `Execute` / `Bye` from
    /// the peer until the peer sends `Bye` or disconnects.
    pub async fn serve_one(&self, listener: &TcpListener) -> WireResult<()> {
        let (stream, _addr) = listener.accept().await?;
        self.handle_connection(stream).await
    }

    /// ADR 0330 — accept one TCP connection, wrap it in
    /// a rustls TlsStream via the supplied acceptor, then
    /// dispatch through the same handler as `serve_one`.
    pub async fn serve_one_tls(
        &self, listener: &TcpListener, acceptor: &TlsAcceptor,
    ) -> WireResult<()> {
        let (stream, _addr) = listener.accept().await?;
        let tls = acceptor.accept(stream).await
            .map_err(|e| WireError::Tls(format!("accept: {e}")))?;
        self.handle_connection(tls).await
    }

    /// ADR 0333 — accept one mTLS connection and derive
    /// the authenticated `Principal` from the verified
    /// peer certificate before dispatching the wire
    /// handler. The `mapper` is called with the leaf
    /// certificate's Subject CN; refusal (`None`) or an
    /// empty peer chain fails the connection with
    /// `WireError::Auth`.
    ///
    /// The underlying `acceptor` must be an mTLS acceptor
    /// built from a `ServerConfig::with_client_cert_verifier`
    /// (ADR 0283 `load_pem_with_client_ca`); otherwise the
    /// peer chain will be empty and the call errors out
    /// before the wire handshake runs.
    pub async fn serve_one_mtls<M>(
        &self, listener: &TcpListener, acceptor: &TlsAcceptor,
        mapper: M,
    ) -> WireResult<bradian_security::Principal>
    where
        M: FnOnce(&str) -> Option<bradian_security::Principal>,
    {
        let (stream, _addr) = listener.accept().await?;
        let tls = acceptor.accept(stream).await
            .map_err(|e| WireError::Tls(format!("accept: {e}")))?;

        // Extract the peer certificate chain BEFORE
        // dispatching any wire bytes. If the acceptor was
        // built with `with_no_client_auth` (not mTLS), the
        // chain is empty and we refuse — this slice is
        // specifically the mTLS entrypoint.
        let peer_certs: Vec<Vec<u8>> = {
            let session_ref = tls.get_ref();
            match session_ref.1.peer_certificates() {
                Some(chain) if !chain.is_empty() =>
                    chain.iter().map(|c| c.as_ref().to_vec()).collect(),
                _ => return Err(WireError::Auth(
                    "mTLS requires a peer certificate chain".into())),
            }
        };
        let principal = bradian_security::principal_from_peer_chain(
            &peer_certs, mapper,
        ).map_err(|e| WireError::Auth(format!("{e}")))?;

        self.handle_connection_with_authz(
            tls, Some(principal.id.clone()), |_| true,
        ).await?;
        Ok(principal)
    }

    /// ADR 0334 — same as `serve_one_mtls`, but each
    /// `Execute` is gated by `authz(&principal, &sql)`.
    /// A `false` return from `authz` replies `ErrorMsg
    /// { code: 3 }` and keeps the session alive so the
    /// peer can try other statements.
    pub async fn serve_one_mtls_authz<M, A>(
        &self, listener: &TcpListener, acceptor: &TlsAcceptor,
        mapper: M, mut authz: A,
    ) -> WireResult<bradian_security::Principal>
    where
        M: FnOnce(&str) -> Option<bradian_security::Principal>,
        A: FnMut(&bradian_security::Principal, &str) -> bool,
    {
        let (stream, _addr) = listener.accept().await?;
        let tls = acceptor.accept(stream).await
            .map_err(|e| WireError::Tls(format!("accept: {e}")))?;
        let peer_certs: Vec<Vec<u8>> = {
            let session_ref = tls.get_ref();
            match session_ref.1.peer_certificates() {
                Some(chain) if !chain.is_empty() =>
                    chain.iter().map(|c| c.as_ref().to_vec()).collect(),
                _ => return Err(WireError::Auth(
                    "mTLS requires a peer certificate chain".into())),
            }
        };
        let principal = bradian_security::principal_from_peer_chain(
            &peer_certs, mapper,
        ).map_err(|e| WireError::Auth(format!("{e}")))?;

        let p_ref = principal.clone();
        let id    = principal.id.clone();
        self.handle_connection_with_authz(
            tls, Some(id), move |sql| authz(&p_ref, sql),
        ).await?;
        Ok(principal)
    }

    async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
        &self, stream: S,
    ) -> WireResult<()> {
        self.handle_connection_with_authz(stream, None, |_| true).await
    }

    /// Dispatch a connection with an authz predicate run
    /// against every `Execute`. Returning `false` from
    /// the predicate replies `ErrorMsg { code: 3 }` and
    /// keeps the session alive so the peer can try other
    /// statements. `principal_id` seeds the audit entries
    /// (ADR 0339) with the authenticated identity; `None`
    /// records entries as `anonymous`.
    async fn handle_connection_with_authz<S, F>(
        &self, mut stream: S, principal_id: Option<String>, mut authz: F,
    ) -> WireResult<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
        F: FnMut(&str) -> bool,
    {
        match read_frame(&mut stream).await? {
            WireMessage::Hello { .. } => {}
            other => return Err(WireError::Handshake(format!("expected Hello, got {other:?}"))),
        }
        let session = self.next_session.fetch_add(1,
            std::sync::atomic::Ordering::SeqCst) + 1;
        write_frame(&mut stream, &WireMessage::HelloOk { session }).await?;

        let who = principal_id.unwrap_or_else(|| "anonymous".to_string());
        self.audit_record(&who, "session_open", &format!("session={session}"), None);

        loop {
            let req = match read_frame(&mut stream).await {
                Ok(m) => m,
                Err(WireError::Closed) => {
                    self.audit_record(&who, "session_close",
                        &format!("session={session}"), Some("eof".into()));
                    return Ok(());
                }
                Err(e) => {
                    self.audit_record(&who, "session_close",
                        &format!("session={session}"), Some(format!("{e}")));
                    return Err(e);
                }
            };
            match req {
                WireMessage::Execute { sql } => {
                    let reply = if !authz(&sql) {
                        self.audit_record(&who, "authz_deny",
                            &audit_target_of(&sql), Some(truncate_sql(&sql)));
                        WireMessage::ErrorMsg {
                            code: 3,
                            message: format!("authorization denied for statement: {sql}"),
                        }
                    } else { match self.engine.execute(&sql) {
                        Ok(SqlResultSet::Rows { columns, rows }) => {
                            // ADR 0338 — typed wire row:
                            // convert each cell via
                            // WireValue::from(Value).
                            let rows: Vec<Vec<WireValue>> = rows.into_iter()
                                .map(|r| r.into_iter().map(WireValue::from).collect())
                                .collect();
                            // ADR 0337 — admit the result
                            // set against the memory
                            // pressure harness before
                            // emission. On refusal, return
                            // a typed ErrorMsg and keep the
                            // session alive.
                            let maybe_guard = if let Some(h) = &self.memory_pressure {
                                let bytes: usize = columns.iter().map(|c| c.len()).sum::<usize>()
                                    + rows.iter().flatten()
                                        .map(wire_value_bytes).sum::<usize>();
                                match h.admit(bytes) {
                                    Ok(g) => Some(g),
                                    Err(e) => {
                                        let msg = format!("{e}");
                                        self.audit_record(&who, "pressure_deny",
                                            &audit_target_of(&sql), Some(truncate_sql(&sql)));
                                        write_frame(&mut stream, &WireMessage::ErrorMsg {
                                            code: 4, message: msg }).await?;
                                        continue;
                                    }
                                }
                            } else { None };
                            let rows_returned = rows.len();
                            let column_types = infer_column_types(&columns, &rows);
                            let reply = WireMessage::RowSet { columns, column_types, rows };
                            write_frame(&mut stream, &reply).await?;
                            drop(maybe_guard);  // release in-flight bytes
                            self.audit_record(&who, "execute_rows",
                                &audit_target_of(&sql),
                                Some(format!("rows={rows_returned}; sql={}",
                                    truncate_sql(&sql))));
                            continue;
                        }
                        Ok(SqlResultSet::Command { affected }) => {
                            self.audit_record(&who, "execute_command",
                                &audit_target_of(&sql),
                                Some(format!("affected={affected}; sql={}",
                                    truncate_sql(&sql))));
                            WireMessage::Command { affected }
                        }
                        Err(e) => {
                            self.audit_record(&who, "execute_error",
                                &audit_target_of(&sql),
                                Some(format!("err={e}; sql={}", truncate_sql(&sql))));
                            WireMessage::ErrorMsg { code: 1, message: format!("{e}") }
                        }
                    }};
                    write_frame(&mut stream, &reply).await?;
                }
                WireMessage::Bye => {
                    write_frame(&mut stream, &WireMessage::Bye).await?;
                    self.audit_record(&who, "session_close",
                        &format!("session={session}"), Some("bye".into()));
                    return Ok(());
                }
                other => {
                    write_frame(&mut stream, &WireMessage::ErrorMsg {
                        code: 2, message: format!("unexpected message: {other:?}") }).await?;
                }
            }
        }
    }

    fn audit_record(&self, principal_id: &str, action: &str, target: &str, detail: Option<String>) {
        if let Some(log) = &self.audit {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64).unwrap_or(0);
            let entry = bradian_security::AuditEntry {
                timestamp_ms: ts,
                principal_id: principal_id.to_string(),
                action: action.to_string(),
                target: target.to_string(),
                plan_fingerprint: None,
                detail,
            };
            // Audit write failures are logged, not
            // propagated: losing the connection because the
            // audit log is temporarily unwritable is the
            // wrong trade-off for operators.
            if let Err(e) = log.record(&entry) {
                tracing::warn!(error = %e, principal = %principal_id,
                    action = %action, "audit log write failed");
            }
        }
    }
}

/// Clip SQL text to a reasonable audit-detail length so a
/// long literal doesn't bloat the log. 256 bytes is plenty
/// for action attribution; the full text belongs in the
/// engine's query log.
fn truncate_sql(sql: &str) -> String {
    const MAX: usize = 256;
    if sql.len() <= MAX { sql.to_string() }
    else {
        let mut s = sql[..MAX].to_string();
        s.push('…');
        s
    }
}

/// Derive a crude "target" for the audit entry from the
/// SQL statement verb. Not a parse — just a prefix so
/// filtering by action class is tractable.
fn audit_target_of(sql: &str) -> String {
    let trimmed = sql.trim_start();
    let verb = trimmed.split_ascii_whitespace().next().unwrap_or("?")
        .to_ascii_uppercase();
    format!("sql:{verb}")
}

// ---------------------------------------------------------------------------
// Client

/// Generic client parameterised by the underlying stream.
/// Use the `WireClient` / `WireTlsClient` type aliases for
/// the two shipped variants.
#[derive(Debug)]
pub struct WireClientT<S> {
    stream:  S,
    session: u64,
}

pub type WireClient    = WireClientT<TcpStream>;
pub type WireTlsClient = WireClientT<tokio_rustls::client::TlsStream<TcpStream>>;

impl<S: AsyncRead + AsyncWrite + Unpin> WireClientT<S> {
    pub fn session(&self) -> u64 { self.session }

    pub async fn execute(&mut self, sql: &str) -> WireResult<WireMessage> {
        write_frame(&mut self.stream, &WireMessage::Execute { sql: sql.into() }).await?;
        read_frame(&mut self.stream).await
    }

    pub async fn bye(mut self) -> WireResult<()> {
        write_frame(&mut self.stream, &WireMessage::Bye).await?;
        match read_frame(&mut self.stream).await {
            Ok(WireMessage::Bye) => Ok(()),
            Ok(other)             => Err(WireError::Unexpected(other)),
            Err(WireError::Closed) => Ok(()),
            Err(e)                => Err(e),
        }
    }
}

async fn handshake_client<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S, principal: String,
) -> WireResult<WireClientT<S>> {
    write_frame(&mut stream, &WireMessage::Hello {
        version: 1, principal }).await?;
    let reply = read_frame(&mut stream).await?;
    let session = match reply {
        WireMessage::HelloOk { session } => session,
        other => return Err(WireError::Handshake(format!("expected HelloOk, got {other:?}"))),
    };
    Ok(WireClientT { stream, session })
}

impl WireClient {
    /// Plaintext TCP connect + handshake.
    pub async fn connect(
        addr: std::net::SocketAddr, principal: impl Into<String>,
    ) -> WireResult<Self> {
        let stream = TcpStream::connect(addr).await?;
        handshake_client(stream, principal.into()).await
    }
}

impl WireTlsClient {
    /// ADR 0330 — TLS-wrapped connect. `connector` is a
    /// caller-configured rustls client (so the caller
    /// chooses the root-cert store, ALPN, SNI, etc.);
    /// `server_name` is the SNI name to send (also used
    /// for cert-subject matching).
    pub async fn connect_tls(
        addr: std::net::SocketAddr,
        server_name: ServerName<'static>,
        connector: &TlsConnector,
        principal: impl Into<String>,
    ) -> WireResult<Self> {
        let tcp = TcpStream::connect(addr).await?;
        let tls = connector.connect(server_name, tcp).await
            .map_err(|e| WireError::Tls(format!("connect: {e}")))?;
        handshake_client(tls, principal.into()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fresh_engine() -> (tempfile::TempDir, Arc<SqlEngine>) {
        let dir = tempdir().unwrap();
        let engine = SqlEngine::open(dir.path()).unwrap();
        for sql in [
            "CREATE DATABASE t",
            "CREATE SCHEMA t.public",
            "CREATE TABLE t.public.items (id BIGINT NOT NULL, value BIGINT, PRIMARY KEY(id))",
            "INSERT INTO t.public.items(id, value) VALUES(1, 100)",
            "INSERT INTO t.public.items(id, value) VALUES(2, 200)",
        ] { engine.execute(sql).unwrap(); }
        (dir, Arc::new(engine))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tcp_round_trip_runs_sql_through_the_engine() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });

        let mut client = WireClient::connect(addr, "alice").await.unwrap();
        assert_eq!(client.session(), 1);
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        match reply {
            WireMessage::RowSet { columns, rows, .. } => {
                assert_eq!(columns.len(), 1);
                assert_eq!(rows.len(),    1);
            }
            other => panic!("expected RowSet, got {other:?}"),
        }
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sql_error_is_forwarded_as_error_msg() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "bob").await.unwrap();
        let reply = client.execute("THIS IS NOT SQL").await.unwrap();
        assert!(matches!(reply, WireMessage::ErrorMsg { .. }),
            "got {reply:?}");
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn multi_execute_in_one_session_works() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "carol").await.unwrap();
        for id in [1, 2, 1] {
            let sql = format!("SELECT value FROM t.public.items WHERE id = {id}");
            let reply = client.execute(&sql).await.unwrap();
            assert!(matches!(reply, WireMessage::RowSet { .. }), "got {reply:?}");
        }
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn oversized_frame_is_rejected_on_read() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut stream = TcpStream::connect(addr).await.unwrap();
        stream.write_all(&(MAX_FRAME_BYTES + 1).to_be_bytes()).await.unwrap();
        stream.flush().await.unwrap();
        let outcome = server_handle.await.unwrap();
        assert!(matches!(outcome, Err(WireError::FrameTooLarge { .. })),
            "got {outcome:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn handshake_rejects_non_hello_first_message() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut stream = TcpStream::connect(addr).await.unwrap();
        write_frame(&mut stream, &WireMessage::Execute {
            sql: "SELECT value FROM t.public.items WHERE id = 1".into() })
            .await.unwrap();
        let outcome = server_handle.await.unwrap();
        assert!(matches!(outcome, Err(WireError::Handshake(_))), "got {outcome:?}");
    }

    // ---------- ADR 0330: TLS-wrapped wire tests ----------

    use tokio_rustls::rustls::{
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName},
        ClientConfig, RootCertStore, ServerConfig,
    };

    fn gen_tls_fixture() -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let cert_der = CertificateDer::from(cert.cert.der().to_vec());
        let key_der  = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            cert.key_pair.serialize_der()));
        (vec![cert_der], key_der)
    }

    fn build_server_config(chain: Vec<CertificateDer<'static>>, key: PrivateKeyDer<'static>)
        -> Arc<ServerConfig>
    {
        rustls::crypto::ring::default_provider().install_default().ok();
        Arc::new(ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .expect("rustls server config"))
    }

    fn build_client_config(trust: &[CertificateDer<'static>]) -> Arc<ClientConfig> {
        rustls::crypto::ring::default_provider().install_default().ok();
        let mut roots = RootCertStore::empty();
        for c in trust { roots.add(c.clone()).unwrap(); }
        Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tls_round_trip_runs_sql_through_the_engine() {
        let (_dir, engine) = fresh_engine();
        let (chain, key)   = gen_tls_fixture();
        let server_cfg = build_server_config(chain.clone(), key);
        let client_cfg = build_client_config(&chain);
        let acceptor   = TlsAcceptor::from(server_cfg);
        let connector  = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move { s.serve_one_tls(&listener, &acc).await }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let mut client = WireTlsClient::connect_tls(
            addr, name, &connector, "alice").await.unwrap();
        assert_eq!(client.session(), 1);
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        match reply {
            WireMessage::RowSet { columns, rows, .. } => {
                assert_eq!(columns.len(), 1);
                assert_eq!(rows.len(),    1);
            }
            other => panic!("expected RowSet, got {other:?}"),
        }
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn tls_client_with_untrusted_server_cert_is_rejected() {
        let (_dir, engine) = fresh_engine();
        let (chain, key)   = gen_tls_fixture();
        let server_cfg = build_server_config(chain, key);
        let acceptor   = TlsAcceptor::from(server_cfg);
        // A DIFFERENT fixture → client doesn't trust the
        // server's cert.
        let (other_chain, _) = gen_tls_fixture();
        let client_cfg = build_client_config(&other_chain);
        let connector  = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move { s.serve_one_tls(&listener, &acc).await }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let err = WireTlsClient::connect_tls(
            addr, name, &connector, "alice").await.unwrap_err();
        assert!(matches!(err, WireError::Tls(_)),
            "expected TLS verification error, got {err:?}");
        // The server's handshake future also surfaces an
        // error (or Closed). Drain it so the task isn't
        // left dangling.
        let _ = server_handle.await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plaintext_client_against_tls_server_is_rejected() {
        let (_dir, engine) = fresh_engine();
        let (chain, key)   = gen_tls_fixture();
        let server_cfg = build_server_config(chain, key);
        let acceptor   = TlsAcceptor::from(server_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move { s.serve_one_tls(&listener, &acc).await }
        });
        // Dial as plaintext — the TLS handshake on the
        // server side rejects the non-ClientHello bytes.
        let mut stream = TcpStream::connect(addr).await.unwrap();
        // Send what a plaintext client would start with:
        // a full Hello frame.
        write_frame(&mut stream, &WireMessage::Hello {
            version: 1, principal: "bob".into() }).await.unwrap();
        let outcome = server_handle.await.unwrap();
        assert!(matches!(outcome, Err(WireError::Tls(_))),
            "plaintext bytes into TLS acceptor should surface Tls error, got {outcome:?}");
    }

    // ---------- ADR 0331: mTLS wire tests ----------
    //
    // These exercise the composition: ADR 0283's
    // `load_pem_with_client_ca` builds a WebPkiClientVerifier-
    // backed ServerConfig; ADR 0330's `serve_one_tls`
    // accepts any TlsAcceptor; the client supplies a cert
    // via rustls::ClientConfig::with_client_auth_cert.

    use tokio_rustls::rustls::server::WebPkiClientVerifier;

    fn build_mtls_server_config(
        server_chain: Vec<CertificateDer<'static>>,
        server_key:   PrivateKeyDer<'static>,
        client_ca:    Vec<CertificateDer<'static>>,
    ) -> Arc<ServerConfig> {
        rustls::crypto::ring::default_provider().install_default().ok();
        let mut roots = RootCertStore::empty();
        for c in client_ca { roots.add(c).unwrap(); }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build().expect("mtls verifier");
        Arc::new(ServerConfig::builder()
            .with_client_cert_verifier(verifier)
            .with_single_cert(server_chain, server_key)
            .expect("mtls server config"))
    }

    fn build_mtls_client_config(
        server_trust: &[CertificateDer<'static>],
        client_chain: Vec<CertificateDer<'static>>,
        client_key:   PrivateKeyDer<'static>,
    ) -> Arc<ClientConfig> {
        rustls::crypto::ring::default_provider().install_default().ok();
        let mut roots = RootCertStore::empty();
        for c in server_trust { roots.add(c.clone()).unwrap(); }
        Arc::new(ClientConfig::builder()
            .with_root_certificates(roots)
            .with_client_auth_cert(client_chain, client_key)
            .expect("mtls client config"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mtls_round_trip_accepts_valid_client_cert() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_chain, client_key) = gen_tls_fixture();

        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_chain.clone());
        let client_cfg = build_mtls_client_config(
            &server_chain, client_chain, client_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move { s.serve_one_tls(&listener, &acc).await }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let mut client = WireTlsClient::connect_tls(
            addr, name, &connector, "alice").await.unwrap();
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(reply, WireMessage::RowSet { .. }), "{reply:?}");
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mtls_server_rejects_client_without_a_cert() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_trust, _unused)    = gen_tls_fixture();

        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_trust);
        // Client config has NO client cert.
        let client_cfg = build_client_config(&server_chain);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move { s.serve_one_tls(&listener, &acc).await }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let outcome = WireTlsClient::connect_tls(
            addr, name, &connector, "alice").await;
        assert!(outcome.is_err(),
            "mTLS server must refuse a cert-less client; got {outcome:?}");
        let server_outcome = server_handle.await.unwrap();
        assert!(matches!(server_outcome, Err(WireError::Tls(_))),
            "server-side task should surface Tls; got {server_outcome:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mtls_server_rejects_client_cert_from_untrusted_ca() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (trusted_client_ca, _)     = gen_tls_fixture();
        // Client presents a DIFFERENT cert.
        let (rogue_chain, rogue_key)   = gen_tls_fixture();

        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, trusted_client_ca);
        let client_cfg = build_mtls_client_config(
            &server_chain, rogue_chain, rogue_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move { s.serve_one_tls(&listener, &acc).await }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let outcome = WireTlsClient::connect_tls(
            addr, name, &connector, "mallory").await;
        assert!(outcome.is_err(),
            "mTLS server must refuse an untrusted client cert; got {outcome:?}");
        let server_outcome = server_handle.await.unwrap();
        assert!(matches!(server_outcome, Err(WireError::Tls(_))),
            "server-side task should surface Tls; got {server_outcome:?}");
    }

    // ---------- ADR 0333: serve_one_mtls → cert-derived principal ----------

    use bradian_security::{Principal, Role};

    /// Variant of `gen_tls_fixture` that forces a specific
    /// CommonName on the self-signed cert so the principal
    /// mapper has something deterministic to key on.
    fn gen_tls_fixture_with_cn(cn: &str)
        -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)
    {
        let mut params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        params.distinguished_name.push(rcgen::DnType::CommonName, cn);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let cert_der = CertificateDer::from(cert.der().to_vec());
        let key_der  = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
            key.serialize_der()));
        (vec![cert_der], key_der)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mtls_derives_principal_from_leaf_cert_cn() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_chain, client_key) = gen_tls_fixture_with_cn("bradian-admin-alice");

        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_chain.clone());
        let client_cfg = build_mtls_client_config(
            &server_chain, client_chain, client_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move {
                s.serve_one_mtls(&listener, &acc, |cn| {
                    cn.strip_prefix("bradian-admin-").map(|id| Principal {
                        id: id.into(),
                        role: Role::Administrator,
                        ou_scope: vec![],
                    })
                }).await
            }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let mut client = WireTlsClient::connect_tls(
            addr, name, &connector, "ignored-by-server").await.unwrap();
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(reply, WireMessage::RowSet { .. }), "{reply:?}");
        client.bye().await.unwrap();

        let principal = server_handle.await.unwrap().unwrap();
        assert_eq!(principal.id, "alice");
        assert_eq!(principal.role, Role::Administrator);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mtls_server_rejects_when_mapper_refuses_the_cn() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_chain, client_key) = gen_tls_fixture_with_cn("unknown-principal");

        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_chain.clone());
        let client_cfg = build_mtls_client_config(
            &server_chain, client_chain, client_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move {
                s.serve_one_mtls(&listener, &acc, |cn| {
                    cn.strip_prefix("bradian-admin-").map(|id| Principal {
                        id: id.into(), role: Role::Administrator, ou_scope: vec![],
                    })
                }).await
            }
        });

        let name = ServerName::try_from("localhost").unwrap();
        // The TLS handshake succeeds; the mapper then
        // refuses on the CN, so the wire handshake never
        // runs. The client's connect_tls either errors
        // during handshake framing OR completes the TLS
        // handshake and gets an EOF when the server drops.
        let _ = WireTlsClient::connect_tls(addr, name, &connector, "x").await;

        let outcome = server_handle.await.unwrap();
        assert!(matches!(outcome, Err(WireError::Auth(_))),
            "mapper refusal should surface as Auth error, got {outcome:?}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn serve_one_mtls_refuses_when_underlying_acceptor_is_non_mtls() {
        // If the caller mis-wires a non-mTLS acceptor
        // (with_no_client_auth) into serve_one_mtls, the
        // peer_certificates() call returns None/empty and
        // we fail closed BEFORE any wire bytes land.
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();

        let server_cfg = build_server_config(server_chain.clone(), server_key);
        let client_cfg = build_client_config(&server_chain);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move {
                s.serve_one_mtls(&listener, &acc, |_cn| Some(Principal {
                    id: "whatever".into(), role: Role::System, ou_scope: vec![],
                })).await
            }
        });

        let name = ServerName::try_from("localhost").unwrap();
        let _ = WireTlsClient::connect_tls(addr, name, &connector, "x").await;

        let outcome = server_handle.await.unwrap();
        assert!(matches!(outcome, Err(WireError::Auth(ref m)) if m.contains("peer certificate")),
            "non-mTLS acceptor plumbed into serve_one_mtls should surface Auth; got {outcome:?}");
    }

    // ---------- ADR 0334: serve_one_mtls_authz — role-gated Execute ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authz_admin_can_run_select_and_insert() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_chain, client_key) = gen_tls_fixture_with_cn("bradian-admin-alice");
        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_chain.clone());
        let client_cfg = build_mtls_client_config(
            &server_chain, client_chain, client_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move {
                s.serve_one_mtls_authz(&listener, &acc,
                    |cn| cn.strip_prefix("bradian-admin-").map(|id| Principal {
                        id: id.into(), role: Role::Administrator, ou_scope: vec![],
                    }),
                    |p, _sql| matches!(p.role, Role::Administrator),
                ).await
            }
        });
        let name = ServerName::try_from("localhost").unwrap();
        let mut client = WireTlsClient::connect_tls(addr, name, &connector, "x").await.unwrap();
        // SELECT allowed.
        let r = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(r, WireMessage::RowSet { .. }), "{r:?}");
        // INSERT also allowed under Admin.
        let r = client.execute(
            "INSERT INTO t.public.items(id, value) VALUES(99, 999)").await.unwrap();
        assert!(matches!(r, WireMessage::Command { affected: 1 }), "{r:?}");
        client.bye().await.unwrap();
        let p = server_handle.await.unwrap().unwrap();
        assert_eq!(p.id, "alice");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn authz_analyst_denied_on_write_but_select_allowed() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_chain, client_key) = gen_tls_fixture_with_cn("bradian-analyst-bob");
        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_chain.clone());
        let client_cfg = build_mtls_client_config(
            &server_chain, client_chain, client_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move {
                s.serve_one_mtls_authz(&listener, &acc,
                    |cn| cn.strip_prefix("bradian-analyst-").map(|id| Principal {
                        id: id.into(), role: Role::Analyst, ou_scope: vec![],
                    }),
                    // Analysts may SELECT; everything else
                    // requires Administrator.
                    |p, sql| {
                        let s = sql.trim_start().to_ascii_lowercase();
                        matches!(p.role, Role::Administrator)
                            || s.starts_with("select")
                    },
                ).await
            }
        });
        let name = ServerName::try_from("localhost").unwrap();
        let mut client = WireTlsClient::connect_tls(addr, name, &connector, "x").await.unwrap();
        // SELECT allowed under Analyst role.
        let r = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(r, WireMessage::RowSet { .. }), "{r:?}");
        // INSERT denied → ErrorMsg code 3, session stays alive.
        let r = client.execute(
            "INSERT INTO t.public.items(id, value) VALUES(100, 1000)").await.unwrap();
        match r {
            WireMessage::ErrorMsg { code, message } => {
                assert_eq!(code, 3);
                assert!(message.contains("authorization denied"));
            }
            other => panic!("expected ErrorMsg(3), got {other:?}"),
        }
        // Session still alive — another SELECT works.
        let r = client.execute(
            "SELECT value FROM t.public.items WHERE id = 2").await.unwrap();
        assert!(matches!(r, WireMessage::RowSet { .. }), "{r:?}");
        client.bye().await.unwrap();
        let p = server_handle.await.unwrap().unwrap();
        assert_eq!(p.id, "bob");
    }

    // ---------- ADR 0337: memory-pressure on RowSet emission ----------

    use bradian_relational::memory_pressure::{
        FixedBudget, MemoryPressureHarness, UnboundedMemory,
    };

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn memory_pressure_refuses_rowset_over_budget() {
        let (_dir, engine) = fresh_engine();
        // Budget too small to admit any real RowSet.
        let harness = Arc::new(MemoryPressureHarness::new(FixedBudget::new(4)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine).with_memory_pressure(harness.clone());
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        match reply {
            WireMessage::ErrorMsg { code, message } => {
                assert_eq!(code, 4);
                assert!(message.contains("budget"),
                    "expected budget-exceeded message, got {message:?}");
            }
            other => panic!("expected ErrorMsg(4), got {other:?}"),
        }
        // Session stays alive.
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
        // Harness recorded one failure.
        let stats = harness.stats();
        assert_eq!(stats.failures, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn memory_pressure_admits_rowset_within_budget() {
        let (_dir, engine) = fresh_engine();
        // Budget comfortably larger than any single RowSet.
        let harness = Arc::new(MemoryPressureHarness::new(FixedBudget::new(1 << 20)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine).with_memory_pressure(harness.clone());
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(reply, WireMessage::RowSet { .. }), "{reply:?}");
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
        let stats = harness.stats();
        assert_eq!(stats.failures, 0,
            "a comfortable budget should admit without refusal");
        assert!(stats.admitted_cumulative > 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn memory_pressure_releases_on_rowset_drop_between_statements() {
        let (_dir, engine) = fresh_engine();
        // Budget big enough for exactly one RowSet. If the
        // release-on-drop is wired correctly, a second
        // statement reuses the budget.
        let harness = Arc::new(MemoryPressureHarness::new(FixedBudget::new(256)));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine).with_memory_pressure(harness.clone());
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let r1 = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(r1, WireMessage::RowSet { .. }));
        let r2 = client.execute(
            "SELECT value FROM t.public.items WHERE id = 2").await.unwrap();
        assert!(matches!(r2, WireMessage::RowSet { .. }));
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
        assert_eq!(harness.in_flight(), 0,
            "all admitted bytes must have been released after the session");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_memory_pressure_rowset_behaviour_is_unchanged() {
        // Baseline: no harness installed → no admission
        // path runs; RowSet is emitted exactly as before.
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);     // no with_memory_pressure
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let r = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        assert!(matches!(r, WireMessage::RowSet { .. }));
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    // ---------- ADR 0338: typed WireValue encoding ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rowset_carries_typed_wire_values_not_debug_strings() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        match reply {
            WireMessage::RowSet { columns, rows, .. } => {
                assert_eq!(columns, vec!["value".to_string()]);
                assert_eq!(rows.len(), 1);
                // The fixture inserts `value = 100` as a
                // BIGINT, which lands on the wire as
                // WireValue::Int64.
                match &rows[0][0] {
                    WireValue::Int64 { value } => assert_eq!(*value, 100),
                    other => panic!("expected Int64(100), got {other:?}"),
                }
            }
            other => panic!("expected RowSet, got {other:?}"),
        }
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[test]
    fn wire_value_from_every_relational_variant_is_lossless_for_the_wire_shape() {
        use bradian_relational::typed_table::Value as R;
        assert!(matches!(WireValue::from(R::Null), WireValue::Null));
        assert!(matches!(WireValue::from(R::Boolean(true)),
            WireValue::Bool { value: true }));
        assert!(matches!(WireValue::from(R::Int32(-1)),
            WireValue::Int32 { value: -1 }));
        assert!(matches!(WireValue::from(R::Int64(1 << 40)),
            WireValue::Int64 { value } if value == 1i64 << 40));
        match WireValue::from(R::Decimal(123_456_789i128)) {
            WireValue::Decimal { value } => assert_eq!(value, "123456789"),
            other => panic!("decimal variant wrong: {other:?}"),
        }
        match WireValue::from(R::String("hello".into())) {
            WireValue::String { value } => assert_eq!(value, "hello"),
            other => panic!("string variant wrong: {other:?}"),
        }
        match WireValue::from(R::TimestampTz(1_700_000_000_000_000)) {
            WireValue::TimestampTz { micros } =>
                assert_eq!(micros, 1_700_000_000_000_000),
            other => panic!("ts variant wrong: {other:?}"),
        }
        match WireValue::from(R::Uuid([0xab; 16])) {
            WireValue::Uuid { value } => {
                assert_eq!(value, "abababab-abab-abab-abab-abababababab");
            }
            other => panic!("uuid variant wrong: {other:?}"),
        }
        match WireValue::from(R::Json("{\"a\":1}".into())) {
            WireValue::Json { value } => assert_eq!(value, "{\"a\":1}"),
            other => panic!("json variant wrong: {other:?}"),
        }
    }

    #[test]
    fn wire_value_json_tag_round_trips_through_serde() {
        // Clients parse WireValue via serde; verify the
        // tag names are stable so a language-level
        // deserialiser in another language can match on
        // "type":"int64" and the "value" field.
        let v = WireValue::Int64 { value: 42 };
        let json = serde_json::to_string(&v).unwrap();
        assert!(json.contains("\"type\":\"int64\""));
        assert!(json.contains("\"value\":42"));
        let back: WireValue = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
    }

    // ---------- ADR 0339: audit-log integration ----------

    use bradian_security::AuditLog;

    fn read_audit(path: &std::path::Path) -> Vec<serde_json::Value> {
        let bytes = std::fs::read_to_string(path).unwrap();
        bytes.lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn audit_records_session_open_close_and_execute() {
        let (_dir, engine) = fresh_engine();
        let logdir = tempdir().unwrap();
        let log_path = logdir.path().join("audit.log");
        let audit = Arc::new(AuditLog::open(log_path.clone()).unwrap());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine).with_audit_log(audit);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let _ = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();

        let entries = read_audit(&log_path);
        // open + execute_rows + close = 3 entries.
        assert!(entries.len() >= 3,
            "expected ≥ 3 audit entries, got {}: {entries:?}",
            entries.len());
        let actions: Vec<String> = entries.iter()
            .map(|e| e["action"].as_str().unwrap().to_string())
            .collect();
        assert!(actions.contains(&"session_open".to_string()));
        assert!(actions.contains(&"execute_rows".to_string()));
        assert!(actions.contains(&"session_close".to_string()));
        // Anonymous since plaintext path.
        for entry in &entries {
            assert_eq!(entry["principal_id"], "anonymous");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn audit_authz_denial_records_authz_deny_entry() {
        let (_dir, engine) = fresh_engine();
        let (server_chain, server_key) = gen_tls_fixture();
        let (client_chain, client_key) = gen_tls_fixture_with_cn("bradian-analyst-bob");
        let server_cfg = build_mtls_server_config(
            server_chain.clone(), server_key, client_chain.clone());
        let client_cfg = build_mtls_client_config(
            &server_chain, client_chain, client_key);
        let acceptor  = TlsAcceptor::from(server_cfg);
        let connector = TlsConnector::from(client_cfg);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let logdir = tempdir().unwrap();
        let log_path = logdir.path().join("audit.log");
        let audit = Arc::new(AuditLog::open(log_path.clone()).unwrap());
        let server = WireServer::new(engine).with_audit_log(audit);
        let server_handle = tokio::spawn({
            let s = server.clone();
            let acc = acceptor.clone();
            async move {
                s.serve_one_mtls_authz(&listener, &acc,
                    |cn| cn.strip_prefix("bradian-analyst-").map(|id| Principal {
                        id: id.into(), role: Role::Analyst, ou_scope: vec![],
                    }),
                    |p, sql| {
                        let s = sql.trim_start().to_ascii_lowercase();
                        matches!(p.role, Role::Administrator)
                            || s.starts_with("select")
                    },
                ).await
            }
        });
        let name = ServerName::try_from("localhost").unwrap();
        let mut client = WireTlsClient::connect_tls(
            addr, name, &connector, "x").await.unwrap();
        // Denied INSERT.
        let _ = client.execute(
            "INSERT INTO t.public.items(id, value) VALUES(777, 777)").await.unwrap();
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();

        let entries = read_audit(&log_path);
        let deny: Vec<_> = entries.iter()
            .filter(|e| e["action"] == "authz_deny").collect();
        assert_eq!(deny.len(), 1, "expected exactly one authz_deny entry");
        assert_eq!(deny[0]["principal_id"], "bob");
        assert_eq!(deny[0]["target"], "sql:INSERT");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn audit_pressure_denial_records_pressure_deny_entry() {
        let (_dir, engine) = fresh_engine();
        let harness = Arc::new(MemoryPressureHarness::new(FixedBudget::new(4)));
        let logdir = tempdir().unwrap();
        let log_path = logdir.path().join("audit.log");
        let audit = Arc::new(AuditLog::open(log_path.clone()).unwrap());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine)
            .with_memory_pressure(harness.clone())
            .with_audit_log(audit);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let _ = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();

        let entries = read_audit(&log_path);
        let deny: Vec<_> = entries.iter()
            .filter(|e| e["action"] == "pressure_deny").collect();
        assert_eq!(deny.len(), 1);
        assert_eq!(deny[0]["target"], "sql:SELECT");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_audit_log_no_entries_are_emitted() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // No with_audit_log call.
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let _ = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
        // Nothing to verify directly — this is a smoke
        // test that the no-audit path doesn't panic.
    }

    #[test]
    fn audit_sql_truncation_caps_at_256_bytes_with_ellipsis() {
        let long = "x".repeat(400);
        let truncated = super::truncate_sql(&long);
        assert!(truncated.len() < 400);
        assert!(truncated.ends_with('…'));
        assert_eq!(super::truncate_sql("SELECT 1"), "SELECT 1");
    }

    #[test]
    fn audit_target_extracts_verb_regardless_of_whitespace_or_case() {
        assert_eq!(super::audit_target_of("SELECT * FROM t"), "sql:SELECT");
        assert_eq!(super::audit_target_of("  insert into x"), "sql:INSERT");
        assert_eq!(super::audit_target_of(""), "sql:?");
    }

    // ---------- ADR 0340: per-column type hints ----------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rowset_carries_inferred_column_types() {
        let (_dir, engine) = fresh_engine();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = WireServer::new(engine);
        let server_handle = tokio::spawn({
            let s = server.clone();
            async move { s.serve_one(&listener).await }
        });
        let mut client = WireClient::connect(addr, "a").await.unwrap();
        let reply = client.execute(
            "SELECT value FROM t.public.items WHERE id = 1").await.unwrap();
        match reply {
            WireMessage::RowSet { columns, column_types, rows } => {
                assert_eq!(columns, vec!["value".to_string()]);
                assert_eq!(rows.len(), 1);
                assert_eq!(column_types.len(), 1);
                assert_eq!(column_types[0], WireType::Int64);
            }
            other => panic!("expected RowSet, got {other:?}"),
        }
        client.bye().await.unwrap();
        server_handle.await.unwrap().unwrap();
    }

    #[test]
    fn wire_value_wire_type_discriminant_is_stable() {
        assert_eq!(WireValue::Null.wire_type(), WireType::Unknown);
        assert_eq!(WireValue::Bool { value: true }.wire_type(), WireType::Bool);
        assert_eq!(WireValue::Int32 { value: 0 }.wire_type(), WireType::Int32);
        assert_eq!(WireValue::Int64 { value: 0 }.wire_type(), WireType::Int64);
        assert_eq!(WireValue::Decimal { value: "0".into() }.wire_type(), WireType::Decimal);
        assert_eq!(WireValue::String { value: "".into() }.wire_type(), WireType::String);
        assert_eq!(WireValue::TimestampTz { micros: 0 }.wire_type(), WireType::TimestampTz);
        assert_eq!(WireValue::Uuid { value: "".into() }.wire_type(), WireType::Uuid);
        assert_eq!(WireValue::Json { value: "{}".into() }.wire_type(), WireType::Json);
    }

    #[test]
    fn infer_column_types_skips_nulls_and_picks_first_typed_cell() {
        let columns = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let rows = vec![
            vec![WireValue::Null, WireValue::Null, WireValue::String { value: "x".into() }],
            vec![WireValue::Int64 { value: 1 }, WireValue::Null, WireValue::String { value: "y".into() }],
            vec![WireValue::Int64 { value: 2 }, WireValue::Bool { value: true }, WireValue::String { value: "z".into() }],
        ];
        let types = infer_column_types(&columns, &rows);
        assert_eq!(types, vec![WireType::Int64, WireType::Bool, WireType::String]);
    }

    #[test]
    fn infer_column_types_all_null_column_stays_unknown() {
        let columns = vec!["x".to_string(), "y".to_string()];
        let rows = vec![
            vec![WireValue::Int64 { value: 1 }, WireValue::Null],
            vec![WireValue::Int64 { value: 2 }, WireValue::Null],
        ];
        assert_eq!(
            infer_column_types(&columns, &rows),
            vec![WireType::Int64, WireType::Unknown],
        );
    }

    #[test]
    fn infer_column_types_empty_rowset_yields_all_unknown() {
        let columns = vec!["a".to_string(), "b".to_string()];
        assert_eq!(
            infer_column_types(&columns, &[]),
            vec![WireType::Unknown, WireType::Unknown],
        );
    }

    #[test]
    fn wire_type_serialisation_is_snake_case() {
        let t = WireType::TimestampTz;
        let json = serde_json::to_string(&t).unwrap();
        assert_eq!(json, "\"timestamp_tz\"");
        let back: WireType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn unbounded_memory_pressure_never_refuses() {
        // Direct unit check: an UnboundedMemory harness
        // wired into the server never bumps the failures
        // counter.
        let harness = MemoryPressureHarness::new(UnboundedMemory::default());
        let g = harness.admit(1 << 24).unwrap();
        drop(g);
        assert_eq!(harness.stats().failures, 0);
    }
}
