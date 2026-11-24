//! ADR 0283 + ADR 0284 — TLS / mTLS config loading tests.
//! Uses `rcgen` to generate ephemeral self-signed certs so
//! no external cert management is required.

use bradian_serving::{load_pem, load_pem_with_client_ca, TlsAcceptorState};
use rcgen::{CertificateParams, DnType, KeyPair};
use tempfile::tempdir;

fn write_self_signed(dir: &std::path::Path, cn: &str)
    -> (std::path::PathBuf, std::path::PathBuf)
{
    let mut params = CertificateParams::new(vec![cn.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, cn);
    let kp = KeyPair::generate().unwrap();
    let cert = params.self_signed(&kp).unwrap();
    let cert_path = dir.join(format!("{cn}.cert.pem"));
    let key_path = dir.join(format!("{cn}.key.pem"));
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, kp.serialize_pem()).unwrap();
    (cert_path, key_path)
}

#[test]
fn load_pem_round_trip() {
    let dir = tempdir().unwrap();
    let (cert, key) = write_self_signed(dir.path(), "localhost");
    let material = load_pem(&cert, &key).unwrap();
    assert_eq!(material.cert_chain.len(), 1);
}

#[test]
fn load_pem_missing_cert() {
    let dir = tempdir().unwrap();
    let ghost = dir.path().join("ghost.pem");
    let (_, key) = write_self_signed(dir.path(), "localhost");
    let err = load_pem(&ghost, &key).unwrap_err();
    assert!(err.to_string().contains("cert file"), "got {err}");
}

#[test]
fn load_pem_missing_key() {
    let dir = tempdir().unwrap();
    let (cert, _) = write_self_signed(dir.path(), "localhost");
    let ghost = dir.path().join("ghost.key.pem");
    let err = load_pem(&cert, &ghost).unwrap_err();
    assert!(err.to_string().contains("key file"), "got {err}");
}

#[test]
fn load_pem_empty_cert_rejected() {
    let dir = tempdir().unwrap();
    let cert = dir.path().join("empty.pem");
    std::fs::write(&cert, b"# no cert blocks\n").unwrap();
    let (_, key) = write_self_signed(dir.path(), "localhost");
    let err = load_pem(&cert, &key).unwrap_err();
    assert!(err.to_string().contains("no certificate"), "got {err}");
}

#[test]
fn acceptor_current_config_matches_load() {
    let dir = tempdir().unwrap();
    let (cert, key) = write_self_signed(dir.path(), "localhost");
    let state = TlsAcceptorState::load(&cert, &key).unwrap();
    let cfg = state.current_config();
    // ServerConfig is opaque; just verify we got one and
    // reload-when-unchanged returns false.
    let _ = cfg;
    assert!(!state.reload_if_changed().unwrap());
}

#[test]
fn acceptor_reloads_on_mtime_change() {
    let dir = tempdir().unwrap();
    let (cert, key) = write_self_signed(dir.path(), "localhost");
    let state = TlsAcceptorState::load(&cert, &key).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    // Re-generate the cert → mtime advances.
    let (cert2, key2) = write_self_signed(dir.path(), "localhost2");
    std::fs::rename(&cert2, &cert).unwrap();
    std::fs::rename(&key2, &key).unwrap();
    assert!(state.reload_if_changed().unwrap());
}

#[test]
fn load_with_client_ca_builds_mtls_config() {
    let dir = tempdir().unwrap();
    let (cert, key) = write_self_signed(dir.path(), "server");
    let (ca_cert, _ca_key) = write_self_signed(dir.path(), "client-ca");
    let material = load_pem_with_client_ca(&cert, &key, &ca_cert).unwrap();
    assert_eq!(material.cert_chain.len(), 1);
}

#[test]
fn load_with_client_ca_rejects_empty_ca() {
    let dir = tempdir().unwrap();
    let (cert, key) = write_self_signed(dir.path(), "server");
    let ca_cert = dir.path().join("empty-ca.pem");
    std::fs::write(&ca_cert, b"# not a cert\n").unwrap();
    let err = load_pem_with_client_ca(&cert, &key, &ca_cert).unwrap_err();
    assert!(err.to_string().contains("no certificate"), "got {err}");
}
