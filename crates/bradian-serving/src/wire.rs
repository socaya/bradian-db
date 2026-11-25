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
    #[error("tls: {0}")]
    Tls(String),
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

    async fn handle_connection<S: AsyncRead + AsyncWrite + Unpin>(
        &self, mut stream: S,
    ) -> WireResult<()> {
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
}
