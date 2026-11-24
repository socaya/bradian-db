//! ADR 0316 — Relational wire protocol + reference driver.
//!
//! A real tokio-based TCP transport for Bradian SQL
//! sessions. Framing is a 4-byte big-endian length prefix
//! followed by a JSON-encoded `WireMessage`. The reference
//! `WireServer` binds to a `TcpListener`, accepts
//! connections, performs the Hello → HelloOk handshake,
//! and dispatches `Execute` to a bound
//! `bradian_relational::SqlEngine`. `WireClient` is a
//! thin tokio-side counterpart that dials, handshakes,
//! executes, and closes.
//!
//! TLS binding (ADR 0283 ships rustls under the HTTP
//! stack) is not wired here. The transport is explicitly
//! cleartext TCP — fine for in-process tests + trusted
//! control planes; a TLS wrapper is a dedicated follow-up.

use std::io;
use std::sync::Arc;

use bradian_relational::sql::{SqlEngine, SqlResultSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Max frame size (1 MiB). Refuse anything larger so a
/// buggy / hostile peer cannot allocate unbounded memory
/// on the far side.
pub const MAX_FRAME_BYTES: u32 = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireMessage {
    Hello    { version: u32, principal: String },
    HelloOk  { session: u64 },
    Execute  { sql: String },
    RowSet   { columns: Vec<String>, rows: Vec<Vec<String>> },
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
        }
    }

    /// Accept one connection from `listener`, drive the
    /// handshake, then loop reading `Execute` / `Bye` from
    /// the peer until the peer sends `Bye` or disconnects.
    pub async fn serve_one(&self, listener: &TcpListener) -> WireResult<()> {
        let (stream, _addr) = listener.accept().await?;
        self.handle_connection(stream).await
    }

    async fn handle_connection(&self, mut stream: TcpStream) -> WireResult<()> {
        match read_frame(&mut stream).await? {
            WireMessage::Hello { .. } => {}
            other => return Err(WireError::Handshake(format!("expected Hello, got {other:?}"))),
        }
        let session = self.next_session.fetch_add(1,
            std::sync::atomic::Ordering::SeqCst) + 1;
        write_frame(&mut stream, &WireMessage::HelloOk { session }).await?;

        loop {
            let req = match read_frame(&mut stream).await {
                Ok(m) => m,
                Err(WireError::Closed) => return Ok(()),
                Err(e) => return Err(e),
            };
            match req {
                WireMessage::Execute { sql } => {
                    let reply = match self.engine.execute(&sql) {
                        Ok(SqlResultSet::Rows { columns, rows }) => {
                            let rows: Vec<Vec<String>> = rows.into_iter()
                                .map(|r| r.into_iter().map(|v| format!("{v:?}")).collect())
                                .collect();
                            WireMessage::RowSet { columns, rows }
                        }
                        Ok(SqlResultSet::Command { affected }) =>
                            WireMessage::Command { affected },
                        Err(e) => WireMessage::ErrorMsg {
                            code: 1, message: format!("{e}") },
                    };
                    write_frame(&mut stream, &reply).await?;
                }
                WireMessage::Bye => {
                    write_frame(&mut stream, &WireMessage::Bye).await?;
                    return Ok(());
                }
                other => {
                    write_frame(&mut stream, &WireMessage::ErrorMsg {
                        code: 2, message: format!("unexpected message: {other:?}") }).await?;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Client

#[derive(Debug)]
pub struct WireClient {
    stream:  TcpStream,
    session: u64,
}

impl WireClient {
    pub async fn connect(
        addr: std::net::SocketAddr, principal: impl Into<String>,
    ) -> WireResult<Self> {
        let mut stream = TcpStream::connect(addr).await?;
        write_frame(&mut stream, &WireMessage::Hello {
            version: 1, principal: principal.into() }).await?;
        let reply = read_frame(&mut stream).await?;
        let session = match reply {
            WireMessage::HelloOk { session } => session,
            other => return Err(WireError::Handshake(format!("expected HelloOk, got {other:?}"))),
        };
        Ok(Self { stream, session })
    }

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
            WireMessage::RowSet { columns, rows } => {
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
}
