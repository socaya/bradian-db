//! Principals and roles.

use dap_core::Uid;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Role {
    /// Full read access across the catalog.
    Administrator,
    /// Read-only within an explicit OU scope.
    Analyst,
    /// Appears in audit logs; never authorized to run queries by itself.
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Principal {
    pub id: String,
    pub role: Role,
    /// Root OU UIDs the principal may read. Empty = unrestricted (administrator).
    pub ou_scope: Vec<Uid>,
}

impl Principal {
    pub fn is_unrestricted(&self) -> bool {
        self.ou_scope.is_empty() && matches!(self.role, Role::Administrator)
    }

    /// Stable hash used to key context-safe query result caches.
    pub fn context_fingerprint(&self) -> dap_core::Fingerprint {
        let mut buf = Vec::new();
        buf.extend_from_slice(self.id.as_bytes());
        buf.push(0);
        buf.extend_from_slice(match self.role {
            Role::Administrator => b"admin",
            Role::Analyst => b"analyst",
            Role::System => b"system",
        });
        buf.push(0);
        let mut scope: Vec<&str> = self.ou_scope.iter().map(|u| u.as_str()).collect();
        scope.sort();
        for s in scope {
            buf.extend_from_slice(s.as_bytes());
            buf.push(0);
        }
        dap_core::Fingerprint::of(&buf)
    }
}
