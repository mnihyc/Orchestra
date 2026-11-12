//! Ordinary administration of the actual system target, separate from private-resource override.
//!
//! Only producers whose target is the system use this predicate. A scoped caller's
//! admission is fresh, exact operation evidence from the shared operation gate; it is
//! neither a new role nor a substitute for the account's current administrator standing.

use orch_access_contract::key_scope::{KeyIdentityKind, KeyTargetKind};

use crate::{Caller, Principal};

fn scoped(caller: &Caller, standing: bool, business_kind: KeyTargetKind) -> bool {
    if !standing || caller.as_admin() {
        return false;
    }
    let Some(grant) = caller.grant() else {
        return false;
    };
    if grant.operation_scope().is_none() || grant.requires_legacy_compatibility() {
        return false;
    }
    let Some(facets) = caller.admitted_facets().filter(|facets| !facets.is_empty()) else {
        return false;
    };
    let selection = &facets[0].operation;
    !selection.is_empty()
        && facets.iter().all(|facet| &facet.operation == selection)
        && caller.permits_facets(facets)
        && facets.iter().any(|facet| {
            facet.resource_kind == business_kind
                && facet.target.object.kind == KeyTargetKind::Identity(KeyIdentityKind::System)
                && facet.target.object.id.is_none()
        })
}

/// Whether the current operation may administer the system itself. Browser controls
/// are unchanged. Resource Frame/ownership/private-inspection predicates never use this.
#[must_use]
pub fn on_system(principal: &Principal, business_kind: KeyTargetKind) -> bool {
    principal.is_system_admin()
        || scoped(
            principal.caller(),
            principal.has_system_admin_standing(),
            business_kind,
        )
}

/// The ordinary administrator authority of the system defaults page. Action labels
/// constrain the complete outer operation; a read preview still runs normal change
/// validation inside its rollback trial and must not become a second role hierarchy.
#[must_use]
pub fn system_defaults(principal: &Principal) -> bool {
    on_system(principal, KeyTargetKind::Identity(KeyIdentityKind::System))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TokenGrant;
    use orch_access_contract::key_scope::{
        KeyAction, KeyFacet, KeyObjectRef, KeyOperation, KeyReach, KeyResolvedTarget, KeyRule,
        KeyScope, KeyTarget,
    };
    use orch_types::Id;

    fn caller(action: KeyAction, target: KeyObjectRef) -> Caller {
        let operation = if action == KeyAction::Read {
            "AdminDefaultsPreview"
        } else {
            "AdminDefaultsUpdate"
        };
        let scope = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: target.kind,
                    id: target.id,
                    reach: KeyReach::SelfOnly,
                },
                resources: None,
                allow: vec![KeyOperation {
                    operation: operation.into(),
                    actions: vec![action],
                }],
            }],
            ..KeyScope::default()
        };
        let grant = TokenGrant::from_stored_scope(&[], None, Some(scope), None).unwrap();
        Caller::token(Id::from_u128(1), grant)
            .admit_facets(&[KeyFacet {
                operation: operation.into(),
                action,
                resource_kind: target.kind,
                target: KeyResolvedTarget {
                    credential_account: None,
                    object: target,
                    node_ancestors: Vec::new(),
                    document_ancestors: Vec::new(),
                },
            }])
            .unwrap()
            .into_caller()
    }

    #[test]
    fn system_authority_requires_current_standing_and_exact_operation_admission() {
        let system = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::System),
            id: None,
        };
        let manage = caller(KeyAction::Manage, system);
        assert!(scoped(&manage, true, system.kind));
        assert!(!scoped(&manage, false, system.kind));
        let read = caller(KeyAction::Read, system);
        assert!(scoped(&read, true, system.kind));
        let mutation = KeyFacet {
            operation: "AdminDefaultsUpdate".into(),
            action: KeyAction::Manage,
            resource_kind: system.kind,
            target: KeyResolvedTarget {
                credential_account: None,
                object: system,
                node_ancestors: Vec::new(),
                document_ancestors: Vec::new(),
            },
        };
        assert!(!read.permits_facets(&[mutation]));
        assert!(!scoped(
            &caller(
                KeyAction::Manage,
                KeyObjectRef {
                    kind: KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org),
                    id: Some(Id::from_u128(2)),
                }
            ),
            true,
            system.kind
        ));
        assert!(!manage.as_admin());
        assert!(!manage.carries_system_admin());
    }

    #[test]
    fn legacy_and_unadmitted_keys_do_not_gain_system_authority() {
        let legacy = Caller::token(
            Id::from_u128(1),
            TokenGrant::from_stored(&["*".into()], None).unwrap(),
        );
        assert!(!scoped(
            &legacy,
            true,
            KeyTargetKind::Identity(KeyIdentityKind::System)
        ));
        let unadmitted = Caller::token(
            Id::from_u128(1),
            TokenGrant::from_stored_scope(&[], None, Some(KeyScope::default()), None).unwrap(),
        );
        assert!(!scoped(
            &unadmitted,
            true,
            KeyTargetKind::Identity(KeyIdentityKind::System)
        ));
    }
}
