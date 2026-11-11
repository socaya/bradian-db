//! Encryption-at-rest primitives (SEC-003).
//!
//! AEAD = ChaCha20-Poly1305. We expose a `DataKey` (32 bytes) + `KeyRing` holding one or
//! more keys identified by a short `KeyId`. Each sealed payload stores the key ID in its
//! header; decryption re-looks it up from the ring. Key rotation is explicit — callers add
//! the new key to the ring, mark it `primary`, and leave the old one as a decrypt-only
//! fallback until a full re-encrypt sweep completes.
//!
//! The crate does NOT implement a key-management-service (KMS). Keys live in the
//! operator's secret store; the engine only receives already-decrypted key material.

use ahash::AHashMap;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

/// 32-byte symmetric key material.
#[derive(Clone)]
pub struct DataKey([u8; 32]);

impl DataKey {
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }

    pub fn generate() -> Self {
        let mut buf = [0u8; 32];
        // chacha20poly1305 re-exports rand_core's OsRng which we use indirectly via
        // `generate_key`. Produce 32 bytes via the AEAD helper and copy them out.
        let key = ChaCha20Poly1305::generate_key(&mut OsRng);
        buf.copy_from_slice(key.as_slice());
        Self(buf)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for DataKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DataKey").field("len", &32).finish()
    }
}

/// Short opaque identifier for a key. Rotating creates a new ID.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeyId(pub String);

impl KeyId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Key ring. Thread-safe via `parking_lot::RwLock`; keys are stored in RAM and never
/// persisted. Callers can serialise and store them encrypted with an operator KMS.
#[derive(Debug, Default)]
pub struct KeyRing {
    primary: parking_lot::RwLock<Option<KeyId>>,
    keys: parking_lot::RwLock<AHashMap<KeyId, DataKey>>,
}

impl KeyRing {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_key(&self, id: KeyId, key: DataKey, make_primary: bool) {
        self.keys.write().insert(id.clone(), key);
        if make_primary {
            *self.primary.write() = Some(id);
        }
    }

    pub fn promote(&self, id: KeyId) -> DapResult<()> {
        if !self.keys.read().contains_key(&id) {
            return Err(DapError::NotFound(format!("unknown key id {id:?}")));
        }
        *self.primary.write() = Some(id);
        Ok(())
    }

    pub fn primary_id(&self) -> Option<KeyId> {
        self.primary.read().clone()
    }

    fn key(&self, id: &KeyId) -> DapResult<DataKey> {
        self.keys
            .read()
            .get(id)
            .cloned()
            .ok_or_else(|| DapError::NotFound(format!("unknown key id {id:?}")))
    }

    pub fn len(&self) -> usize {
        self.keys.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.keys.read().is_empty()
    }
}

/// On-disk envelope for a sealed payload. The byte layout is:
///   u8 version (= 1)
///   u8 key_id_len
///   u8[key_id_len] key_id
///   u8[12]  nonce
///   u8[]   ciphertext || tag (AEAD output)
#[derive(Clone, Debug)]
pub struct SealedPayload(pub Vec<u8>);

/// Encrypt `plaintext` using the primary key. The associated-data value binds the sealed
/// payload to its context (segment file name, log name, etc.) so an attacker cannot move
/// a sealed blob to a different file without decryption failing.
pub fn seal(ring: &KeyRing, plaintext: &[u8], aad: &[u8]) -> DapResult<SealedPayload> {
    let id = ring
        .primary_id()
        .ok_or_else(|| DapError::invariant("key ring has no primary key"))?;
    let key = ring.key(&id)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ct = cipher
        .encrypt(
            &nonce,
            chacha20poly1305::aead::Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| DapError::other(format!("encrypt: {e}")))?;

    let mut out = Vec::with_capacity(2 + id.as_str().len() + 12 + ct.len());
    out.push(1u8); // version
    let id_bytes = id.as_str().as_bytes();
    out.push(id_bytes.len() as u8);
    out.extend_from_slice(id_bytes);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(SealedPayload(out))
}

/// Decrypt a sealed payload. The caller must supply the same AAD used at seal time.
pub fn unseal(ring: &KeyRing, sealed: &SealedPayload, aad: &[u8]) -> DapResult<Vec<u8>> {
    let bytes = &sealed.0;
    if bytes.len() < 2 + 12 {
        return Err(DapError::corruption("sealed payload truncated"));
    }
    if bytes[0] != 1 {
        return Err(DapError::unsupported(format!(
            "sealed payload version {} not supported",
            bytes[0]
        )));
    }
    let id_len = bytes[1] as usize;
    if bytes.len() < 2 + id_len + 12 {
        return Err(DapError::corruption("sealed payload header truncated"));
    }
    let id_bytes = &bytes[2..2 + id_len];
    let id = KeyId::new(std::str::from_utf8(id_bytes).map_err(|e| {
        DapError::corruption(format!("key id not utf8: {e}"))
    })?);
    let nonce_off = 2 + id_len;
    let nonce = Nonce::from_slice(&bytes[nonce_off..nonce_off + 12]);
    let ct = &bytes[nonce_off + 12..];
    let key = ring.key(&id)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(key.as_bytes()));
    cipher
        .decrypt(nonce, chacha20poly1305::aead::Payload { msg: ct, aad })
        .map_err(|e| DapError::other(format!("decrypt: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_with_aad() {
        let ring = KeyRing::new();
        ring.add_key(KeyId::new("k1"), DataKey::generate(), true);
        let sealed = seal(&ring, b"patients.csv rows", b"seg-1.derc").unwrap();
        let plain = unseal(&ring, &sealed, b"seg-1.derc").unwrap();
        assert_eq!(plain, b"patients.csv rows");
    }

    #[test]
    fn mismatched_aad_fails_decryption() {
        let ring = KeyRing::new();
        ring.add_key(KeyId::new("k1"), DataKey::generate(), true);
        let sealed = seal(&ring, b"secret", b"ctxA").unwrap();
        assert!(unseal(&ring, &sealed, b"ctxB").is_err());
    }

    #[test]
    fn rotated_key_still_decrypts_old_payload() {
        let ring = KeyRing::new();
        ring.add_key(KeyId::new("k1"), DataKey::generate(), true);
        let sealed = seal(&ring, b"legacy", b"aad").unwrap();
        // Rotate: new primary.
        ring.add_key(KeyId::new("k2"), DataKey::generate(), true);
        assert_eq!(ring.primary_id().unwrap().as_str(), "k2");
        // Old key still present, so legacy payload decrypts.
        let plain = unseal(&ring, &sealed, b"aad").unwrap();
        assert_eq!(plain, b"legacy");
        // New encryption uses k2.
        let sealed2 = seal(&ring, b"fresh", b"aad").unwrap();
        // First bytes: version=1, then key_id_len=2, then "k2".
        assert_eq!(&sealed2.0[2..4], b"k2");
    }

    #[test]
    fn missing_primary_key_refuses_seal() {
        let ring = KeyRing::new();
        assert!(seal(&ring, b"x", b"aad").is_err());
    }
}
