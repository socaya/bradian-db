//! ADR 0248 — WAL-backed, replay-validated user catalog journal.
//!
//! A user mutation is applied to a private clone of the in-memory
//! `UserCatalog`, then durably appended as ONE BRWL mutation. Only
//! after WAL sync succeeds does that new state become visible. On
//! restart, every mutation is replayed through the original
//! `UserCatalog` validators (identifier shape, duplicate / not-found
//! checks). Argon2id hashing happens at commit time, not at replay —
//! the hash is persisted in the event so reopens are deterministic.
//!
//! This is a separate journal from `PersistentCatalog`. Cross-journal
//! atomicity (e.g. GRANT referencing a user) is not yet designed;
//! call it out as a known limitation in any ADR that spans both.

use std::path::Path;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::row_store::{RowStore, StoreError};
use crate::user_catalog::{UserCatalog, UserCatalogError};

const MARKER_KEY: &[u8] = b"bradian.users.version";
const MARKER_VALUE: &[u8] = b"BRADIAN-USERS-V1";
const EVENT_PREFIX: &str = "user/";
const JOURNAL_VERSION: u8 = 1;

#[derive(Debug, Error)]
pub enum PersistentUserCatalogError {
    #[error(transparent)]
    User(#[from] UserCatalogError),
    #[error(transparent)]
    Storage(#[from] StoreError),
    #[error(transparent)]
    Encoding(#[from] serde_json::Error),
    #[error("user journal is corrupt: {0}")]
    Corruption(String),
}

pub type PersistentUserCatalogResult<T> = Result<T, PersistentUserCatalogError>;

/// ADR 0248 — journaled event. argon2id hashing has already
/// run; the stored `password_hash` is the PHC-encoded string.
/// SetActive is a bit flip. DropUser references the canonical
/// identifier, matching `UserCatalog::drop_user`.
#[derive(Debug, Clone, Serialize, Deserialize)]
enum Command {
    CreateUser { name: String, password_hash: String },
    DropUser { name: String },
    SetPassword { name: String, password_hash: String },
    SetActive { name: String, active: bool },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Event { version: u8, command: Command }

fn event_key(lsn: u64) -> String {
    format!("{EVENT_PREFIX}{lsn:016x}")
}

/// Apply a journaled mutation to an in-memory catalog. This
/// bypasses the argon2 hashing in the normal API — the stored
/// hash is reused verbatim, so reopens are deterministic and
/// don't pay the KDF cost per user per boot.
fn apply_event(cat: &mut UserCatalog, event: &Event) -> PersistentUserCatalogResult<()> {
    match &event.command {
        Command::CreateUser { name, password_hash } => {
            cat.insert_hashed(name, password_hash, /*active=*/true)
                .map_err(PersistentUserCatalogError::User)
        }
        Command::DropUser { name } => {
            cat.drop_user(name).map_err(PersistentUserCatalogError::User)
        }
        Command::SetPassword { name, password_hash } => {
            cat.replace_hash(name, password_hash)
                .map_err(PersistentUserCatalogError::User)
        }
        Command::SetActive { name, active } => {
            cat.set_active(name, *active).map_err(PersistentUserCatalogError::User)
        }
    }
}

/// One exclusively owned durable user catalog. Mutations
/// serialize under a mutex and are WAL-synced before the new
/// in-memory state is published.
#[derive(Debug)]
pub struct PersistentUserCatalog {
    inner: Mutex<Inner>,
}

#[derive(Debug)]
struct Inner {
    catalog: UserCatalog,
    journal: RowStore,
}

impl PersistentUserCatalog {
    /// Open the journal at `path`. Replays every stored event
    /// through the UserCatalog validators. An empty journal is
    /// initialized with the version marker.
    pub fn open(path: impl AsRef<Path>) -> PersistentUserCatalogResult<Self> {
        let journal = RowStore::open(path.as_ref())?;
        let mut catalog = UserCatalog::new();
        let rows = journal.scan();
        if rows.is_empty() {
            journal.put(MARKER_KEY, MARKER_VALUE)?;
        } else {
            if journal.get(MARKER_KEY).as_deref() != Some(MARKER_VALUE) {
                return Err(PersistentUserCatalogError::Corruption(
                    "user journal version marker is absent or incompatible".into()));
            }
            let mut next_lsn = 2_u64;
            for (key, value) in &rows {
                if key.as_slice() == MARKER_KEY { continue; }
                let expected_key = event_key(next_lsn);
                if key != expected_key.as_bytes() {
                    return Err(PersistentUserCatalogError::Corruption(format!(
                        "missing, duplicated, or unexpected user event at LSN {next_lsn}")));
                }
                let event: Event = serde_json::from_slice(value).map_err(|e|
                    PersistentUserCatalogError::Corruption(
                        format!("invalid user event at LSN {next_lsn}: {e}")))?;
                if event.version != JOURNAL_VERSION {
                    return Err(PersistentUserCatalogError::Corruption(
                        format!("unsupported user event version at LSN {next_lsn}")));
                }
                apply_event(&mut catalog, &event).map_err(|e|
                    PersistentUserCatalogError::Corruption(
                        format!("user replay failed at LSN {next_lsn}: {e}")))?;
                next_lsn = next_lsn.checked_add(1).ok_or_else(||
                    PersistentUserCatalogError::Corruption(
                        "user event sequence overflow".into()))?;
            }
            if journal.last_lsn() != next_lsn - 1 {
                return Err(PersistentUserCatalogError::Corruption(
                    "user journal LSN does not match event count".into()));
            }
        }
        Ok(Self { inner: Mutex::new(Inner { catalog, journal }) })
    }

    /// Commit one event after applying it to a tentative clone
    /// so corrupt / duplicate / not-found errors never reach
    /// the journal.
    fn commit(&self, command: Command) -> PersistentUserCatalogResult<u64> {
        let mut inner = self.inner.lock();
        let mut tentative = inner.catalog.clone();
        let event = Event { version: JOURNAL_VERSION, command };
        apply_event(&mut tentative, &event)?;
        let next_lsn = inner.journal.last_lsn().checked_add(1)
            .ok_or(PersistentUserCatalogError::Storage(StoreError::SequenceExhausted))?;
        let bytes = serde_json::to_vec(&event)?;
        let actual = inner.journal.put(event_key(next_lsn).as_bytes(), &bytes)?;
        if actual != next_lsn {
            return Err(PersistentUserCatalogError::Corruption(
                "user journal recorded unexpected LSN".into()));
        }
        inner.catalog = tentative;
        Ok(actual)
    }

    /// Create a user, hashing the plaintext with argon2id BEFORE
    /// the WAL append so the stored hash is canonical.
    pub fn create_user(&self, name: &str, password: &str) -> PersistentUserCatalogResult<u64> {
        // Reuse UserCatalog::create_user's validation on a scratch
        // instance so hashing failure or duplicate rejection
        // surfaces BEFORE we hash into the journal.
        let mut probe = UserCatalog::new();
        probe.create_user(name, password)?;
        let record = probe.get(name).expect("just created").clone();
        self.commit(Command::CreateUser {
            name: record.name.clone(),
            password_hash: record.password_hash().to_owned(),
        })
    }

    pub fn drop_user(&self, name: &str) -> PersistentUserCatalogResult<u64> {
        self.commit(Command::DropUser { name: name.to_owned() })
    }

    pub fn set_password(&self, name: &str, password: &str) -> PersistentUserCatalogResult<u64> {
        let mut probe = UserCatalog::new();
        // Insert a throwaway record so set_password's hash path runs.
        probe.create_user(name, "x")?;
        probe.set_password(name, password)?;
        let record = probe.get(name).expect("rehashed").clone();
        self.commit(Command::SetPassword {
            name: record.name.clone(),
            password_hash: record.password_hash().to_owned(),
        })
    }

    pub fn set_active(&self, name: &str, active: bool) -> PersistentUserCatalogResult<u64> {
        self.commit(Command::SetActive { name: name.to_owned(), active })
    }

    /// Verify a plaintext password against the current state.
    /// Delegates to UserCatalog so the constant-time comparison
    /// happens in one place.
    pub fn verify_password(&self, name: &str, password: &str) -> PersistentUserCatalogResult<()> {
        let inner = self.inner.lock();
        inner.catalog.verify_password(name, password).map_err(PersistentUserCatalogError::User)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.inner.lock().catalog.contains(name)
    }

    pub fn snapshot(&self) -> UserCatalog { self.inner.lock().catalog.clone() }

    /// Current journal LSN (useful for tests and tooling).
    pub fn last_lsn(&self) -> u64 { self.inner.lock().journal.last_lsn() }
}
