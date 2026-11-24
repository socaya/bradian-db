//! ADR 0247 — In-memory user catalog with argon2id password
//! hashing. First P0-4 foundation slice: no SQL grammar yet,
//! no persistence yet, no GRANT yet. Just the primitive.

use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

use argon2::{Algorithm, Argon2, Params, Version};
use bradian_core::Capability;
use password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand::rngs::OsRng;

use crate::TableId;

/// ADR 0258 — per-object capabilities granted on a specific
/// table. Distinct from `bradian_core::Capability` which is
/// global/coarse (verb-level); this enum is the first slice
/// of a finer-grained per-table authorization model.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash,
    serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ObjectCapability {
    /// `SELECT` on this table — read-only.
    Select,
    /// ADR 0278 — `INSERT` on this table.
    Insert,
    /// ADR 0278 — `UPDATE` on this table.
    Update,
    /// ADR 0278 — `DELETE` on this table.
    Delete,
    /// ADR 0279 — `ALTER` on this table (RENAME / DROP COLUMN /
    /// SET DEFAULT / SET NOT NULL / TYPE / FK / UK).
    Alter,
    /// ADR 0279 — `DROP` on this table.
    Drop,
}

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
    /// ADR 0251 — authorization capabilities granted to this
    /// user. Empty by default; the first user created in an
    /// empty catalog is bootstrapped with all capabilities so
    /// there is always a path to grant others.
    #[serde(default)]
    capabilities: BTreeSet<Capability>,
    /// ADR 0258 — per-table object grants. A user with global
    /// `Capability::Read` is implicitly authorized on every
    /// table; this map only matters for users that lack the
    /// global read capability.
    #[serde(default)]
    table_grants: BTreeMap<TableId, BTreeSet<ObjectCapability>>,
    /// ADR 0281 — roles granted to this user. `effective_*`
    /// methods union the user's own caps / table_grants with
    /// those of each named role.
    #[serde(default)]
    roles: BTreeSet<String>,
}

/// ADR 0281 — a named bundle of capabilities. Keyed in
/// `UserCatalog::roles` by canonical lowercase name.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoleRecord {
    pub name: String,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
    #[serde(default)]
    pub table_grants: BTreeMap<TableId, BTreeSet<ObjectCapability>>,
}

impl UserRecord {
    /// Expose the stored hash for persistence. Verification
    /// must still go through `UserCatalog::verify_password`
    /// so timing-safe comparisons happen in one place.
    pub fn password_hash(&self) -> &str { &self.password_hash }

    /// Capabilities the user may exercise. Empty set means the
    /// user may authenticate but cannot run SQL.
    pub fn capabilities(&self) -> &BTreeSet<Capability> { &self.capabilities }

    /// ADR 0258 — per-table object grants held by this user.
    pub fn table_grants(&self) -> &BTreeMap<TableId, BTreeSet<ObjectCapability>> {
        &self.table_grants
    }

    /// ADR 0281 — roles granted to this user.
    pub fn roles(&self) -> &BTreeSet<String> { &self.roles }

    /// ADR 0258 + ADR 0278 — true if the user may exercise
    /// `cap` on `table`. The global `Capability::Read`
    /// implies `Select`; the global `Capability::Write`
    /// implies `Insert`, `Update`, `Delete`.
    pub fn allows_on_table(&self, table: TableId, cap: ObjectCapability) -> bool {
        let implied_by_global = match cap {
            ObjectCapability::Select =>
                self.capabilities.contains(&Capability::Read),
            ObjectCapability::Insert
            | ObjectCapability::Update
            | ObjectCapability::Delete =>
                self.capabilities.contains(&Capability::Write),
            // ADR 0279 — global Ddl implies table ALTER / DROP.
            ObjectCapability::Alter
            | ObjectCapability::Drop =>
                self.capabilities.contains(&Capability::Ddl),
        };
        if implied_by_global { return true; }
        self.table_grants.get(&table)
            .is_some_and(|caps| caps.contains(&cap))
    }
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
    /// ADR 0281 — named role bundles.
    #[serde(default)]
    roles: BTreeMap<String, RoleRecord>,
}

impl UserCatalog {
    pub fn new() -> Self { Self::default() }

    /// Register a new active user. Rejects invalid names,
    /// duplicates, and empty passwords. Hashes with argon2id.
    /// ADR 0251 — if the catalog is empty at call time, the
    /// new user is bootstrapped with every capability so there
    /// is always a path to grant others. Subsequent users
    /// start with an empty capability set.
    pub fn create_user(&mut self, name: &str, password: &str) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        if password.is_empty() {
            return Err(UserCatalogError::InvalidPassword);
        }
        if self.users.contains_key(&name) {
            return Err(UserCatalogError::Duplicate(name));
        }
        let capabilities: BTreeSet<Capability> = if self.users.is_empty() {
            [Capability::Read, Capability::Write, Capability::Ddl, Capability::Admin]
                .into_iter().collect()
        } else { BTreeSet::new() };
        let phc = hash_password(password)?;
        self.users.insert(name.clone(), UserRecord {
            name, password_hash: phc, active: true, capabilities,
            table_grants: BTreeMap::new(),
            roles: BTreeSet::new(),
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
            .ok_or(UserCatalogError::NotFound(name))?;
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

    /// ADR 0248 — insert a user with an already-hashed PHC
    /// string. Journal replay uses this so re-hashing is not
    /// paid per boot. Rejects invalid names, duplicates, and
    /// empty hashes. ADR 0251 — caller supplies capabilities
    /// (replay carries them verbatim).
    pub(crate) fn insert_hashed(
        &mut self, name: &str, password_hash: &str, active: bool,
        capabilities: BTreeSet<Capability>,
    ) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        if password_hash.is_empty() {
            return Err(UserCatalogError::Hashing("empty hash".into()));
        }
        if self.users.contains_key(&name) {
            return Err(UserCatalogError::Duplicate(name));
        }
        self.users.insert(name.clone(), UserRecord {
            name, password_hash: password_hash.to_owned(), active, capabilities,
            table_grants: BTreeMap::new(),
            roles: BTreeSet::new(),
        });
        Ok(())
    }

    /// ADR 0258 — grant one object capability on one table.
    /// Idempotent. Rejects unknown users.
    pub fn grant_on_table(&mut self, name: &str, table: TableId, cap: ObjectCapability)
        -> UserCatalogResult<()>
    {
        let name = validate_name(name)?;
        let record = self.users.get_mut(&name)
            .ok_or(UserCatalogError::NotFound(name))?;
        record.table_grants.entry(table).or_default().insert(cap);
        Ok(())
    }

    /// ADR 0258 — revoke one object capability on one table.
    /// Idempotent (revoking an absent grant is a no-op).
    /// Rejects unknown users.
    pub fn revoke_on_table(&mut self, name: &str, table: TableId, cap: ObjectCapability)
        -> UserCatalogResult<()>
    {
        let name = validate_name(name)?;
        let record = self.users.get_mut(&name)
            .ok_or(UserCatalogError::NotFound(name))?;
        if let Some(caps) = record.table_grants.get_mut(&table) {
            caps.remove(&cap);
            if caps.is_empty() { record.table_grants.remove(&table); }
        }
        Ok(())
    }

    /// ADR 0281 — create a role with explicit capabilities.
    pub fn create_role(&mut self, name: &str, capabilities: BTreeSet<Capability>)
        -> UserCatalogResult<()>
    {
        let name = validate_name(name)?;
        if let Some(existing) = self.roles.get(&name) {
            if existing.capabilities == capabilities { return Ok(()); }
            return Err(UserCatalogError::Duplicate(name));
        }
        self.roles.insert(name.clone(), RoleRecord {
            name, capabilities, table_grants: BTreeMap::new(),
        });
        Ok(())
    }

    /// ADR 0281 — drop a role. Auto-revokes from every user.
    /// Idempotent.
    pub fn drop_role(&mut self, name: &str) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        if self.roles.remove(&name).is_some() {
            for user in self.users.values_mut() {
                user.roles.remove(&name);
            }
        }
        Ok(())
    }

    /// ADR 0281 — grant a role to a user.
    pub fn grant_role(&mut self, user: &str, role: &str) -> UserCatalogResult<()> {
        let user = validate_name(user)?;
        let role = validate_name(role)?;
        if !self.roles.contains_key(&role) {
            return Err(UserCatalogError::NotFound(role));
        }
        let record = self.users.get_mut(&user)
            .ok_or(UserCatalogError::NotFound(user))?;
        record.roles.insert(role);
        Ok(())
    }

    /// ADR 0281 — revoke a role from a user.
    pub fn revoke_role(&mut self, user: &str, role: &str) -> UserCatalogResult<()> {
        let user = validate_name(user)?;
        let role = validate_name(role)?;
        let record = self.users.get_mut(&user)
            .ok_or(UserCatalogError::NotFound(user))?;
        record.roles.remove(&role);
        Ok(())
    }

    /// ADR 0281 — look up a role.
    pub fn role(&self, name: &str) -> Option<&RoleRecord> {
        let name = validate_name(name).ok()?;
        self.roles.get(&name)
    }

    /// ADR 0281 — iterate every stored role.
    pub fn roles_iter(&self) -> impl Iterator<Item = &RoleRecord> {
        self.roles.values()
    }

    /// ADR 0281 — effective global capabilities (union of
    /// user's own with every role's).
    pub fn effective_capabilities(&self, user: &str) -> BTreeSet<Capability> {
        let mut out = BTreeSet::new();
        if let Ok(name) = validate_name(user) {
            if let Some(record) = self.users.get(&name) {
                out.extend(record.capabilities.iter().copied());
                for role_name in &record.roles {
                    if let Some(role) = self.roles.get(role_name) {
                        out.extend(role.capabilities.iter().copied());
                    }
                }
            }
        }
        out
    }

    /// ADR 0281 — effective per-table grants (union of user's
    /// own with every role's).
    pub fn effective_table_grants(&self, user: &str)
        -> BTreeMap<TableId, BTreeSet<ObjectCapability>>
    {
        let mut out: BTreeMap<TableId, BTreeSet<ObjectCapability>> = BTreeMap::new();
        if let Ok(name) = validate_name(user) {
            if let Some(record) = self.users.get(&name) {
                for (t, caps) in &record.table_grants {
                    out.entry(*t).or_default().extend(caps.iter().copied());
                }
                for role_name in &record.roles {
                    if let Some(role) = self.roles.get(role_name) {
                        for (t, caps) in &role.table_grants {
                            out.entry(*t).or_default().extend(caps.iter().copied());
                        }
                    }
                }
            }
        }
        out
    }

    /// ADR 0251 — grant one capability. Idempotent. Rejects
    /// unknown names.
    pub fn grant(&mut self, name: &str, capability: Capability) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        let record = self.users.get_mut(&name)
            .ok_or(UserCatalogError::NotFound(name))?;
        record.capabilities.insert(capability);
        Ok(())
    }

    /// ADR 0251 — revoke one capability. Idempotent. Rejects
    /// unknown names.
    pub fn revoke(&mut self, name: &str, capability: Capability) -> UserCatalogResult<()> {
        let name = validate_name(name)?;
        let record = self.users.get_mut(&name)
            .ok_or(UserCatalogError::NotFound(name))?;
        record.capabilities.remove(&capability);
        Ok(())
    }

    /// ADR 0248 — replace the stored hash for a user without
    /// re-hashing. Rejects unknown names and empty hashes.
    pub(crate) fn replace_hash(&mut self, name: &str, password_hash: &str)
        -> UserCatalogResult<()>
    {
        let name = validate_name(name)?;
        if password_hash.is_empty() {
            return Err(UserCatalogError::Hashing("empty hash".into()));
        }
        let record = self.users.get_mut(&name)
            .ok_or(UserCatalogError::NotFound(name))?;
        record.password_hash = password_hash.to_owned();
        Ok(())
    }
}
