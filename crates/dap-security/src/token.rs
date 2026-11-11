//! Opaque bearer-token store. Tokens are stored hashed (BLAKE3) so leaking the store does not
//! disclose live tokens. This v1 implementation is in-memory only; a persistent token store
//! lands alongside the admin app.

use ahash::AHashMap;
use parking_lot::RwLock;

use crate::principal::Principal;

#[derive(Debug, Default)]
pub struct TokenStore {
    entries: RwLock<AHashMap<[u8; 32], Principal>>,
}

impl TokenStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, token: &str, principal: Principal) {
        let h = blake3::hash(token.as_bytes());
        self.entries.write().insert(*h.as_bytes(), principal);
    }

    pub fn resolve(&self, token: &str) -> Option<Principal> {
        let h = blake3::hash(token.as_bytes());
        self.entries.read().get(h.as_bytes()).cloned()
    }

    pub fn len(&self) -> usize {
        self.entries.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::principal::Role;

    #[test]
    fn register_and_resolve() {
        let store = TokenStore::new();
        let p = Principal {
            id: "alice".into(),
            role: Role::Analyst,
            ou_scope: vec![dap_core::Uid::new("ouROOT00000").unwrap()],
        };
        store.register("secret-token", p.clone());
        let got = store.resolve("secret-token").unwrap();
        assert_eq!(got.id, "alice");
        assert!(store.resolve("wrong").is_none());
    }
}
