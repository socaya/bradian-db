//! DHIS2-style UID: 11 characters, first must be a letter, remainder alphanumeric.
//!
//! The format is a hard constraint across DHIS2 metadata. We preserve it as a dedicated newtype
//! so that catalog, storage and query layers cannot confuse it with free-form strings.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{DapError, DapResult};

pub const UID_LEN: usize = 11;

/// A validated DHIS2-style identifier.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Uid(String);

impl Uid {
    pub fn new(raw: impl Into<String>) -> DapResult<Self> {
        let raw = raw.into();
        if raw.len() != UID_LEN {
            return Err(DapError::InvalidUid(format!(
                "expected {} chars, got {}",
                UID_LEN,
                raw.len()
            )));
        }
        let mut chars = raw.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() => {}
            _ => return Err(DapError::InvalidUid("first char must be ASCII letter".into())),
        }
        for c in chars {
            if !c.is_ascii_alphanumeric() {
                return Err(DapError::InvalidUid(format!("non-alphanumeric char: {c:?}")));
            }
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Deterministically derive a UID from arbitrary bytes. Useful for synthetic test fixtures.
    /// Not cryptographic — not for identity collisions over adversarial input.
    pub fn derive(bytes: &[u8]) -> Self {
        let digest = blake3::hash(bytes);
        let hex = digest.to_hex();
        let bytes = hex.as_bytes();
        // Force first char to a letter to satisfy the DHIS2 rule.
        let mut out = String::with_capacity(UID_LEN);
        let first = bytes[0];
        let mapped_first = if first.is_ascii_alphabetic() {
            first as char
        } else {
            // map 0..9 -> a..j
            (b'a' + (first - b'0')) as char
        };
        out.push(mapped_first);
        for &b in &bytes[1..UID_LEN] {
            out.push(b as char);
        }
        Self(out)
    }
}

impl fmt::Debug for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Uid({})", self.0)
    }
}

impl fmt::Display for Uid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Uid {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<Uid> for String {
    fn from(u: Uid) -> Self {
        u.0
    }
}

impl TryFrom<String> for Uid {
    type Error = DapError;
    fn try_from(value: String) -> DapResult<Self> {
        Uid::new(value)
    }
}

impl TryFrom<&str> for Uid {
    type Error = DapError;
    fn try_from(value: &str) -> DapResult<Self> {
        Uid::new(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_uid_accepted() {
        let u = Uid::new("abcDEF12345").unwrap();
        assert_eq!(u.as_str(), "abcDEF12345");
    }

    #[test]
    fn rejects_bad_length() {
        assert!(Uid::new("tooShort").is_err());
        assert!(Uid::new("thisUidIsTooLong1").is_err());
    }

    #[test]
    fn rejects_leading_digit() {
        assert!(Uid::new("1abcDEF2345").is_err());
    }

    #[test]
    fn rejects_non_alnum() {
        assert!(Uid::new("abcDEF1234!").is_err());
    }

    #[test]
    fn derive_produces_valid_uid() {
        for payload in ["payload", "", "the-quick-brown-fox"] {
            let u = Uid::derive(payload.as_bytes());
            assert_eq!(u.as_str().len(), UID_LEN);
            // Must round-trip through the validator.
            let _ = Uid::new(u.as_str()).unwrap();
        }
    }

    #[test]
    fn json_roundtrip() {
        let u = Uid::new("abcDEF12345").unwrap();
        let s = serde_json::to_string(&u).unwrap();
        assert_eq!(s, "\"abcDEF12345\"");
        let back: Uid = serde_json::from_str(&s).unwrap();
        assert_eq!(u, back);
    }
}
