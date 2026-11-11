//! Authorization: convert a `Principal` + `Catalog` + query context into a predicate the
//! planner can AND with the user's request.
//!
//! The predicate is applied in the binder before cost estimation, so the executor never
//! receives data the principal lacks authorization for. Output row shapes are unchanged.

use std::sync::Arc;

use dap_catalog::Catalog;
use dap_core::{Fingerprint, Value};
use dap_runtime::expr::{Expr, Predicate};

use crate::principal::{Principal, Role};

#[derive(Clone, Debug)]
pub struct PolicyContext {
    pub principal: Principal,
    pub catalog: Arc<Catalog>,
}

/// Build the OU-scope predicate for a principal: if their scope is non-empty, inject
/// `ou IN (descendants of scope)`. Administrators with empty scopes pass through
/// unrestricted.
pub fn policy_predicate(ctx: &PolicyContext) -> Predicate {
    if ctx.principal.is_unrestricted() {
        return Predicate::True;
    }
    if ctx.principal.ou_scope.is_empty() {
        // Non-admin with empty scope: deny-by-default.
        return Predicate::False;
    }
    if matches!(ctx.principal.role, Role::System) {
        return Predicate::False;
    }
    let mut allowed: ahash::AHashSet<dap_core::Uid> = ahash::AHashSet::default();
    for root in &ctx.principal.ou_scope {
        for u in ctx.catalog.org_unit_subtree(root) {
            allowed.insert(u);
        }
    }
    let values: Vec<Value> = allowed
        .into_iter()
        .map(|u| Value::Str(u.as_str().to_string()))
        .collect();
    Predicate::In {
        expr: Expr::Col("ou".into()),
        values,
    }
}

/// Cache key fingerprint that mixes the principal's context with the plan identity. Ensures
/// result cache entries never leak across users with different OU/field scopes.
pub fn cache_key(
    principal: &Principal,
    plan_fingerprint: Fingerprint,
    metadata_revision: Fingerprint,
) -> Fingerprint {
    let mut buf = Vec::with_capacity(16 * 3);
    buf.extend_from_slice(principal.context_fingerprint().as_bytes());
    buf.extend_from_slice(plan_fingerprint.as_bytes());
    buf.extend_from_slice(metadata_revision.as_bytes());
    Fingerprint::of(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dap_catalog::{Catalog, MetadataObject, OrgUnit};
    use dap_core::Uid;

    fn uid(s: &str) -> Uid {
        Uid::new(s).unwrap()
    }

    fn seed() -> Arc<Catalog> {
        let cat = Catalog::new();
        cat.apply(vec![
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouROOT00000"),
                code: None,
                name: "root".into(),
                parent: None,
                level: 0,
                path: vec![uid("ouROOT00000")],
            }),
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouAAAAAAAAA"),
                code: None,
                name: "A".into(),
                parent: Some(uid("ouROOT00000")),
                level: 1,
                path: vec![uid("ouROOT00000"), uid("ouAAAAAAAAA")],
            }),
            MetadataObject::OrgUnit(OrgUnit {
                uid: uid("ouBBBBBBBBB"),
                code: None,
                name: "B".into(),
                parent: Some(uid("ouROOT00000")),
                level: 1,
                path: vec![uid("ouROOT00000"), uid("ouBBBBBBBBB")],
            }),
        ])
        .unwrap();
        Arc::new(cat)
    }

    #[test]
    fn admin_is_unrestricted() {
        let catalog = seed();
        let p = Principal {
            id: "root".into(),
            role: Role::Administrator,
            ou_scope: vec![],
        };
        let pred = policy_predicate(&PolicyContext { principal: p, catalog });
        assert_eq!(pred, Predicate::True);
    }

    #[test]
    fn analyst_scope_becomes_in_filter() {
        let catalog = seed();
        let p = Principal {
            id: "alice".into(),
            role: Role::Analyst,
            ou_scope: vec![uid("ouROOT00000")],
        };
        let pred = policy_predicate(&PolicyContext { principal: p, catalog });
        match pred {
            Predicate::In { values, .. } => assert_eq!(values.len(), 3),
            other => panic!("expected IN predicate, got {other:?}"),
        }
    }

    #[test]
    fn empty_scope_denies() {
        let catalog = seed();
        let p = Principal {
            id: "alice".into(),
            role: Role::Analyst,
            ou_scope: vec![],
        };
        let pred = policy_predicate(&PolicyContext { principal: p, catalog });
        assert_eq!(pred, Predicate::False);
    }

    #[test]
    fn cache_key_differentiates_principals() {
        let fp = Fingerprint::of(b"plan");
        let meta = Fingerprint::of(b"meta");
        let alice = Principal {
            id: "alice".into(),
            role: Role::Analyst,
            ou_scope: vec![uid("ouAAAAAAAAA")],
        };
        let bob = Principal {
            id: "bob".into(),
            role: Role::Analyst,
            ou_scope: vec![uid("ouAAAAAAAAA")],
        };
        assert_ne!(cache_key(&alice, fp, meta), cache_key(&bob, fp, meta));
    }
}
