//! Content-addressed fingerprint used for schema, metadata and plan identity.
//!
//! BLAKE3 truncated to 128 bits. Hex-encoded 32-char string round-trippable through JSON.

use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Fingerprint([u8; 16]);

impl Fingerprint {
    pub fn of(bytes: &[u8]) -> Self {
        let hash = blake3::hash(bytes);
        let mut out = [0u8; 16];
        out.copy_from_slice(&hash.as_bytes()[..16]);
        Self(out)
    }

    pub fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn as_hex(&self) -> String {
        let mut s = String::with_capacity(32);
        for b in &self.0 {
            use std::fmt::Write;
            let _ = write!(s, "{b:02x}");
        }
        s
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.as_hex())
    }
}
impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_hex())
    }
}
impl From<Fingerprint> for String {
    fn from(f: Fingerprint) -> Self {
        f.as_hex()
    }
}
impl TryFrom<String> for Fingerprint {
    type Error = String;
    fn try_from(s: String) -> Result<Self, String> {
        if s.len() != 32 {
            return Err(format!("fingerprint must be 32 hex chars, got {}", s.len()));
        }
        let mut out = [0u8; 16];
        for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
            let pair = std::str::from_utf8(chunk).map_err(|e| e.to_string())?;
            out[i] = u8::from_str_radix(pair, 16).map_err(|e| e.to_string())?;
        }
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_hash() {
        let a = Fingerprint::of(b"hello");
        let b = Fingerprint::of(b"hello");
        assert_eq!(a, b);
        assert_ne!(a, Fingerprint::of(b"hellp"));
    }

    #[test]
    fn hex_roundtrip() {
        let a = Fingerprint::of(b"dap");
        let s = a.as_hex();
        let b: Fingerprint = s.try_into().unwrap();
        assert_eq!(a, b);
    }
}
