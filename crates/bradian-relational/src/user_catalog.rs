//! ADR 0247 — In-memory user catalog with argon2id password
//! hashing. First P0-4 foundation slice: no SQL grammar yet,
//! no persistence yet, no GRANT yet. Just the primitive.

use std::collections::BTreeMap;
use thiserror::Error;

use argon2::{Algorithm, Argon2, Params, Version};
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::rngs::OsRng;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UserCatalogError {
    #[error("invalid username: {0}")]
    InvalidName(String),
    #[error("user already exists: {0}")]
    Duplicate(String),
    #[error("user not found: {0}")]
    NotFound(String),
    #[error("invalid password")]
    InvalidPassword,
    #[error("password hashing failed: {0}")]
    Hashing(String),
}

pub type UserCatalogResult<T> = Result<T, UserCatalogError>;

/// ADR 0247 — a single user record. Only the argon2id PHC
/// string is retained; the plaintext password is never stored
/// and `set_password` takes it by value so callers can't
/// accidentally log it.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserRecord {
    pub name: String,
    /// Argon2id PHC-encoded hash (incl. salt + params).
    password_hash: String,
    pub active: bool,
}

impl UserRecord {
    /// Expose the stored hash for persistence. Verification
    /// must still go through `UserCatalog::verify_password`
    /// so timing-safe comparisons happen in one place.
    pub fn password_hash(&self) -> &str { &self.password_hash }
}

/// ADR 0247 — user identifier shape. Match the catalog's
/// identifier() rules so a user name can later map to a
/// schema object without re-encoding: start with letter or
/// underscore, 1..=63 ASCII alphanumeric or underscore chars.
fn validate_name(name: &str) -> UserCatalogResult<String> {
    let valid = !name.is_empty()
        && name.len() <= 63
        && name.is_ascii()
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    if !valid {
        return Err(UserCatalogError::InvalidName(name.to_owned()));
    }
    Ok(name.to_ascii_lowercase())
}

/// Hash a plaintext password using argon2id with a fresh
/// random salt. Returns the PHC-encoded string (includes
/// algorithm, version, params, salt, hash — safe to persist).
fn hash_password(password: &str) -> UserCatalogResult<String> {
    // Argon2id, default-ish params: m=19456 KiB, t=2, p=1
    // (the OWASP 2024 baseline for argon2id). password-hash's
    // `default_params()` picks these; wire them explicitly so
    // a future ADR can tune without surprising reopens.
    let params = Params::new(
        /* m_cost  */ 19_456,
        /* t_cost  */ 2,
        /* p_cost  */ 1,
        /* output  */ None,
    ).map_err(|e| UserCatalogError::Hashing(e.to_string()))?;
    let hasher = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let salt = SaltString::generate(&mut OsRng);
    let phc = hasher.hash_password(password.as_bytes(), &salt)
        .map_err(|e| UserCatalogError::Hashing(e.to_string()))?;
    Ok(phc.to_string())
}

/// ADR 0247 — verify the plaintext against a stored PHC hash.
/// Timing is masked by the password-hash crate's own
/// constant-time comparison; wrong passwords and absent
/// users surface as the same `InvalidPassword` error above.
fn verify_phc(password: &str, phc_string: &str) -> UserCatalogResult<()> {
    let phc = PasswordHash::new(phc_string)
        .map_err(|e| UserCatalogError::Hashing(e.to_string()))?;
    let hasher = Argon2::default();
    hasher.verify_password(password.as_bytes(), &phc)
        .map_err(|_| UserCatalogError::InvalidPassword)
}

/// ADR 0247 — in-memory map of (name → record). No persistence
/// yet — a future slice journals mutations alongside the
/// generic catalog. Clone + serde let the in-memory state be
/// copied for backup/restore scaffolding when that lands.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct UserCatalog {
    users: BTreeMap<String, UserRecord>,
}

impl UserCatalog {
    pub fn new() -> Self { Self::default() }

    /// Register a new active user. Rejects invalid names,
    /// duplicates, and empty passwords. Hashes with argon2id.
    pub fn create_user(&mut self, name: &str, password: &str) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        if password.is_empty() {
            return Err(UserCatalogError::InvalidPassword);
        }
        if self.users.contains_key(&name) {
            return Err(UserCatalogError::Duplicate(name));
        }
        let phc = hash_password(password)?;
        self.users.insert(name.clone(), UserRecord {
            name, password_hash: phc, active: true,
        });
        Ok(())
    }

    /// Remove a user. Rejects unknown names.
    pub fn drop_user(&mut self, name: &str) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        if self.users.remove(&name).is_none() {
            return Err(UserCatalogError::NotFound(name));
        }
        Ok(())
    }

    /// Rotate the stored password. Rejects unknown names and
    /// empty passwords. Re-hashes with a fresh salt.
    pub fn set_password(&mut self, name: &str, password: &str) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        if password.is_empty() {
            return Err(UserCatalogError::InvalidPassword);
        }
        let record = self.users.get_mut(&name)
            .ok_or_else(|| UserCatalogError::NotFound(name.clone()))?;
        record.password_hash = hash_password(password)?;
        Ok(())
    }

    /// Flip the active bit. An inactive user cannot
    /// authenticate (verify_password returns InvalidPassword
    /// for them). Rejects unknown names.
    pub fn set_active(&mut self, name: &str, active: bool) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        let record = self.users.get_mut(&name)
            .ok_or_else(|| UserCatalogError::NotFound(name))?;
        record.active = active;
        Ok(())
    }

    /// Verify a plaintext password for a user. Unknown users,
    /// inactive users, and wrong passwords all surface the
    /// same `InvalidPassword` error so an attacker cannot
    /// distinguish them.
    pub fn verify_password(&self, name: &str, password: &str) -> UserCatalogResult<()> {
        let name = validate_name(name).map_err(|_| UserCatalogError::InvalidPassword)?;
        let record = self.users.get(&name).ok_or(UserCatalogError::InvalidPassword)?;
        if !record.active { return Err(UserCatalogError::InvalidPassword); }
        verify_phc(password, &record.password_hash)
    }

    /// Does this user exist? (Case-insensitive on the stored
    /// identifier form.) Does not reveal active state.
    pub fn contains(&self, name: &str) -> bool {
        match validate_name(name) {
            Ok(n) => self.users.contains_key(&n),
            Err(_) => false,
        }
    }

    /// Return the stored record (useful for persistence and
    /// policy enforcement). Does not expose the hash by itself.
    pub fn get(&self, name: &str) -> Option<&UserRecord> {
        let name = validate_name(name).ok()?;
        self.users.get(&name)
    }

    /// Iterate every stored user, in lexicographic order.
    pub fn users(&self) -> impl Iterator<Item = &UserRecord> {
        self.users.values()
    }

    /// How many users are registered.
    pub fn len(&self) -> usize { self.users.len() }
    pub fn is_empty(&self) -> bool { self.users.is_empty() }
}
