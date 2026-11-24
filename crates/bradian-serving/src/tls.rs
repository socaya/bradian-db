//! ADR 0283 — TLS at the API boundary (rustls).
//!
//! `TlsConfig` loads a server certificate chain + private key
//! from PEM files and produces a `rustls::ServerConfig` that
//! an acceptor can wrap TCP sockets with. ADR 0284 extends
//! this with an optional client-cert verifier (mTLS).
//!
//! The current scope is the configuration primitive: load +
//! build + hot-reload. Wiring it into the HTTP server loop
//! (`routes::router()` + an `axum::serve_with` branch) is
//! trivial once the primitive is reachable.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use parking_lot::RwLock;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TlsError {
    #[error("TLS cert file I/O at {path:?}: {source}")]
    CertIo { path: PathBuf, #[source] source: std::io::Error },
    #[error("TLS key file I/O at {path:?}: {source}")]
    KeyIo { path: PathBuf, #[source] source: std::io::Error },
    #[error("TLS cert file at {path:?} contained no certificate blocks")]
    EmptyCert { path: PathBuf },
    #[error("TLS key file at {path:?} contained no private key blocks")]
    EmptyKey { path: PathBuf },
    #[error("TLS key parse: {0}")]
    KeyParse(String),
    #[error("rustls configuration: {0}")]
    Rustls(String),
    #[error("mTLS verifier: {0}")]
    MtlsVerifier(String),
}

pub type TlsResult<T> = Result<T, TlsError>;

/// ADR 0283 — immutable TLS material + the derived
/// `ServerConfig`. The hot-reload wrapper below holds one
/// of these inside an `RwLock` so a file-mtime change can
/// swap it under live connections.
pub struct TlsMaterial {
    pub cert_chain: Vec<CertificateDer<'static>>,
    pub private_key: PrivateKeyDer<'static>,
    pub server_config: Arc<ServerConfig>,
    pub loaded_at: SystemTime,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub cert_mtime: Option<SystemTime>,
    pub key_mtime: Option<SystemTime>,
}

impl std::fmt::Debug for TlsMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsMaterial")
            .field("cert_chain_len", &self.cert_chain.len())
            .field("cert_path", &self.cert_path)
            .field("key_path", &self.key_path)
            .field("loaded_at", &self.loaded_at)
            .finish()
    }
}

/// ADR 0283 — load certificate chain + private key from PEM.
pub fn load_pem(
    cert_path: &Path, key_path: &Path,
) -> TlsResult<TlsMaterial> {
    load_pem_internal(cert_path, key_path, None)
}

/// ADR 0284 — load server cert + key AND require client
/// certificates signed by the provided CA bundle.
pub fn load_pem_with_client_ca(
    cert_path: &Path, key_path: &Path, client_ca_path: &Path,
) -> TlsResult<TlsMaterial> {
    load_pem_internal(cert_path, key_path, Some(client_ca_path))
}

fn load_pem_internal(
    cert_path: &Path, key_path: &Path,
    client_ca_path: Option<&Path>,
) -> TlsResult<TlsMaterial> {
    let cert_file = File::open(cert_path)
        .map_err(|source| TlsError::CertIo { path: cert_path.into(), source })?;
    let mut cert_reader = BufReader::new(cert_file);
    let cert_chain: Vec<CertificateDer<'static>> =
        rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| TlsError::CertIo { path: cert_path.into(), source })?;
    if cert_chain.is_empty() {
        return Err(TlsError::EmptyCert { path: cert_path.into() });
    }

    let key_file = File::open(key_path)
        .map_err(|source| TlsError::KeyIo { path: key_path.into(), source })?;
    let mut key_reader = BufReader::new(key_file);
    let private_key = rustls_pemfile::private_key(&mut key_reader)
        .map_err(|source| TlsError::KeyIo { path: key_path.into(), source })?
        .ok_or_else(|| TlsError::EmptyKey { path: key_path.into() })?;

    let server_config = build_server_config(
        cert_chain.clone(),
        private_key.clone_key(),
        client_ca_path,
    )?;

    let now = SystemTime::now();
    let cert_mtime = std::fs::metadata(cert_path).ok()
        .and_then(|m| m.modified().ok());
    let key_mtime = std::fs::metadata(key_path).ok()
        .and_then(|m| m.modified().ok());
    Ok(TlsMaterial {
        cert_chain, private_key, server_config: Arc::new(server_config),
        loaded_at: now,
        cert_path: cert_path.into(),
        key_path: key_path.into(),
        cert_mtime, key_mtime,
    })
}

fn build_server_config(
    certs: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    client_ca_path: Option<&Path>,
) -> TlsResult<ServerConfig> {
    use rustls::server::WebPkiClientVerifier;
    let builder = ServerConfig::builder();
    let builder = if let Some(ca_path) = client_ca_path {
        // ADR 0284 — load client-trust roots from PEM.
        let f = File::open(ca_path)
            .map_err(|source| TlsError::CertIo { path: ca_path.into(), source })?;
        let mut r = BufReader::new(f);
        let ca_certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut r)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|source| TlsError::CertIo { path: ca_path.into(), source })?;
        if ca_certs.is_empty() {
            return Err(TlsError::EmptyCert { path: ca_path.into() });
        }
        let mut roots = rustls::RootCertStore::empty();
        for c in &ca_certs {
            roots.add(c.clone())
                .map_err(|e| TlsError::MtlsVerifier(e.to_string()))?;
        }
        let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| TlsError::MtlsVerifier(e.to_string()))?;
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    builder
        .with_single_cert(certs, key)
        .map_err(|e| TlsError::Rustls(e.to_string()))
}

/// ADR 0283 — hot-reloadable TLS material. A background
/// poller (or caller-driven `reload_if_changed`) swaps in
/// new `ServerConfig` when either PEM file's mtime advances.
pub struct TlsAcceptorState {
    inner: Arc<RwLock<TlsMaterial>>,
    client_ca_path: Option<PathBuf>,
}

impl TlsAcceptorState {
    pub fn load(cert_path: &Path, key_path: &Path) -> TlsResult<Self> {
        let material = load_pem(cert_path, key_path)?;
        Ok(Self { inner: Arc::new(RwLock::new(material)), client_ca_path: None })
    }

    pub fn load_with_client_ca(
        cert_path: &Path, key_path: &Path, client_ca_path: &Path,
    ) -> TlsResult<Self> {
        let material = load_pem_with_client_ca(cert_path, key_path, client_ca_path)?;
        Ok(Self {
            inner: Arc::new(RwLock::new(material)),
            client_ca_path: Some(client_ca_path.into()),
        })
    }

    pub fn current_config(&self) -> Arc<ServerConfig> {
        Arc::clone(&self.inner.read().server_config)
    }

    /// ADR 0283 — reload the PEM files if either mtime has
    /// advanced since the last load. Returns `Ok(true)` when
    /// a reload actually happened.
    pub fn reload_if_changed(&self) -> TlsResult<bool> {
        let (cert_path, key_path, last_cert_mtime, last_key_mtime) = {
            let m = self.inner.read();
            (m.cert_path.clone(), m.key_path.clone(), m.cert_mtime, m.key_mtime)
        };
        let now_cert = std::fs::metadata(&cert_path).ok()
            .and_then(|m| m.modified().ok());
        let now_key = std::fs::metadata(&key_path).ok()
            .and_then(|m| m.modified().ok());
        if now_cert == last_cert_mtime && now_key == last_key_mtime {
            return Ok(false);
        }
        let new_material = load_pem_internal(
            &cert_path, &key_path, self.client_ca_path.as_deref())?;
        *self.inner.write() = new_material;
        Ok(true)
    }
}
