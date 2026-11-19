//! Ordinary administrative operation admission, separate from browser private-access override.

use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyIdentityKind, KeyTargetKind};
use orch_api_contract::operations::{self, Delegation, FacetUsage, OperationFacet, TargetResolver};
use orch_api_contract::routes::Route;
use orch_authz::Caller;
use orch_types::Id;

use crate::middleware::authentication::{Authed, Via};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;

/// A node configuration effect of this exact ordinary administrative operation.
/// Only the checked constructor can make it, and it borrows the current principal.
pub(crate) struct AdministrativeNode<'a> {
    principal: &'a orch_authz::Principal,
    node: Id,
}

impl AdministrativeNode<'_> {
    pub(crate) fn principal(&self) -> &orch_authz::Principal {
        self.principal
    }
    pub(crate) fn node(&self) -> Id {
        self.node
    }

    /// Only SettingsManage at this actual node is covered. Provider/environment
    /// needs, private reads and system-wide locks retain their existing checks.
    pub(crate) fn covers(&self, need: orch_settings::validation::Need) -> bool {
        covers_node_settings(self.node, need)
    }
}

fn covers_node_settings(target: Id, need: orch_settings::validation::Need) -> bool {
    matches!(need, orch_settings::validation::Need::OnNode { permission: orch_access_contract::permissions::Permission::SettingsManage, node } if node == target)
}

#[derive(Clone, Copy)]
enum Origin {
    Session { as_admin: bool },
    Key { id: Id },
}

#[derive(Clone, Copy)]
struct ActorFacts {
    id: Id,
    system_admin: bool,
    origin: Origin,
}

impl ActorFacts {
    fn of(user: &Authed) -> Self {
        Self {
            id: user.id(),
            system_admin: user.user.is_system_admin && !user.user.suspended(),
            origin: match user.via {
                Via::Session { .. } => Origin::Session {
                    as_admin: user.as_admin,
                },
                Via::Token { token_id, .. } => Origin::Key { id: token_id },
            },
        }
    }
}

fn matches_facet(
    metadata: &OperationFacet,
    actual: &KeyFacet,
    actor: Id,
    path: &impl Fn(&str) -> Option<Id>,
) -> bool {
    metadata.action == actual.action
        && metadata.resources.contains(&actual.resource_kind)
        && match metadata.target {
            TargetResolver::System => {
                actual.target.object.kind == KeyTargetKind::Identity(KeyIdentityKind::System)
                    && actual.target.object.id.is_none()
            }
            TargetResolver::CurrentAccount => {
                actual.target.object.kind == KeyTargetKind::Identity(KeyIdentityKind::Account)
                    && actual.target.object.id == Some(actor)
            }
            TargetResolver::Path(parameter) => {
                path(parameter).is_some_and(|id| actual.target.object.id == Some(id))
            }
            TargetResolver::TypedPath { parameter, kind } => {
                actual.target.object.kind == kind
                    && path(parameter).is_some_and(|id| actual.target.object.id == Some(id))
            }
            // Prepared has already resolved and admitted every governing target on the
            // producer's connection. Repeating a guessed resolver here would be unsafe.
            TargetResolver::PermissionObject
            | TargetResolver::Resolved(_)
            | TargetResolver::ResolvedCollection(_) => true,
        }
}

fn allowed(
    route: &Route,
    actor: ActorFacts,
    caller: &Caller,
    path: impl Fn(&str) -> Option<Id>,
) -> ApiResult<bool> {
    if !actor.system_admin || caller.user() != actor.id {
        return Ok(false);
    }
    match actor.origin {
        Origin::Session { as_admin } => {
            return Ok(as_admin && caller.grant().is_none() && caller.as_admin());
        }
        Origin::Key { id } => {
            let Some(grant) = caller.grant() else {
                return Ok(false);
            };
            if caller.as_admin()
                || grant.operation_scope().is_none()
                || grant.requires_legacy_compatibility()
                || !grant.token_ids().any(|token| token == id)
            {
                return Ok(false);
            }
        }
    }
    let Some(facets) = caller.admitted_facets().filter(|facets| !facets.is_empty()) else {
        return Ok(false);
    };
    if !caller.permits_facets(facets) {
        return Ok(false);
    }
    let operation = operations::for_route(route).map_err(ApiError::internal)?;
    if operation.delegation != Delegation::Business {
        return Ok(false);
    }
    let Some(variant) = operation
        .variants
        .iter()
        .find(|variant| variant.operation == facets[0].operation)
    else {
        return Ok(false);
    };
    Ok(facets.iter().all(|facet| {
        facet.operation == variant.operation
            && variant
                .facets
                .iter()
                .any(|metadata| matches_facet(metadata, facet, actor.id, &path))
    }) && variant
        .facets
        .iter()
        .filter(|metadata| metadata.usage == FacetUsage::Required)
        .all(|metadata| {
            facets
                .iter()
                .any(|facet| matches_facet(metadata, facet, actor.id, &path))
        }))
}

/// The same ordinary administration predicate for an already restored, currently
/// admitted issuer. This returns no caller or native private-administration mode.
pub(crate) fn accepted_system_administration(
    principal: &orch_authz::Principal,
    route: &Route,
) -> ApiResult<bool> {
    let caller = principal.caller();
    let origin = match caller.grant() {
        None => Origin::Session {
            as_admin: caller.as_admin(),
        },
        Some(grant) => {
            let mut ids = grant.token_ids();
            let Some(id) = ids.next() else {
                return Ok(false);
            };
            if ids.next().is_some() {
                return Ok(false);
            }
            Origin::Key { id }
        }
    };
    allowed(
        route,
        ActorFacts {
            id: principal.user(),
            system_admin: principal.has_system_admin_standing(),
            origin,
        },
        caller,
        |_| None,
    )
}

impl Ctx {
    /// Entry to an existing ordinary admin producer. Browser behavior keeps its explicit
    /// mode; an operation-scoped key needs exact current admission and current standing.
    /// This method never changes the caller's global/private administration predicates.
    pub(crate) fn ordinary_system_admin(&self) -> ApiResult<Id> {
        let user = self.user()?;
        let browser;
        let caller = if user.is_session() {
            browser = authorization::caller_of(user)?;
            &browser
        } else {
            self.admitted_caller().ok_or_else(|| {
                refusals::system_admin_only(
                    "this administrative operation is not admitted for the key",
                )
            })?
        };
        self.ordinary_system_admin_with(user, caller)
    }

    /// Recheck the same administrative producer with the guard's fresh identity and
    /// freshly admitted caller. Never substitute the older context user at this checkpoint.
    pub(crate) fn ordinary_system_admin_with(
        &self,
        user: &Authed,
        caller: &Caller,
    ) -> ApiResult<Id> {
        if allowed(&self.route, ActorFacts::of(user), caller, |name| {
            self.id(name).ok()
        })? {
            Ok(user.id())
        } else {
            Err(refusals::system_admin_only(
                "current system administration and admission to this operation are required",
            ))
        }
    }

    /// Bind the admin producer's actual node configuration effect to its current
    /// principal; a different node or an unrelated admitted Read cannot become a write.
    pub(crate) fn administrative_node<'a>(
        &self,
        principal: &'a orch_authz::Principal,
        node: Id,
    ) -> ApiResult<AdministrativeNode<'a>> {
        self.ordinary_system_admin_with(self.user()?, principal.caller())?;
        if !principal.has_system_admin_standing() {
            return Err(refusals::system_admin_only(
                "current system administration is required",
            ));
        }
        if principal.caller().grant().is_some() && !principal.caller().admitted_facets().is_some_and(|facets| facets.iter().any(|facet|
            facet.action == KeyAction::Manage && facet.target.object.id == Some(node)
                && matches!(facet.target.object.kind, KeyTargetKind::Object(kind) if kind.node().is_some()))) {
            return Err(refusals::system_admin_only("the key does not admit this node configuration effect"));
        }
        Ok(AdministrativeNode { principal, node })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::{
        KeyAction, KeyObjectRef, KeyOperation, KeyReach, KeyResolvedTarget, KeyRule, KeyScope,
        KeyTarget,
    };
    use orch_api_contract::routes::Endpoint;
    use orch_authz::TokenGrant;

    fn actor(key: bool) -> ActorFacts {
        ActorFacts {
            id: Id::from_u128(1),
            system_admin: true,
            origin: if key {
                Origin::Key {
                    id: Id::from_u128(2),
                }
            } else {
                Origin::Session { as_admin: true }
            },
        }
    }

    fn key(operation: &str, target: KeyObjectRef, action: KeyAction) -> Caller {
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
        let grant = TokenGrant::from_stored_scope(&[], None, Some(scope), None)
            .unwrap()
            .for_token(Id::from_u128(2));
        let caller = Caller::token(Id::from_u128(1), grant);
        caller
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
    fn browser_mode_and_current_standing_are_preserved() {
        let route = orch_api_contract::admin::AdminSystemGet::ROUTE;
        let browser = Caller::session(actor(false).id).with_admin_mode(true);
        assert!(allowed(&route, actor(false), &browser, |_| None).unwrap());
        let mut without_mode = actor(false);
        without_mode.origin = Origin::Session { as_admin: false };
        assert!(!allowed(&route, without_mode, &browser, |_| None).unwrap());
        let mut withdrawn = actor(false);
        withdrawn.system_admin = false;
        assert!(!allowed(&route, withdrawn, &browser, |_| None).unwrap());
    }

    #[test]
    fn exact_system_read_is_admitted_without_private_override() {
        let route = orch_api_contract::admin::AdminSystemGet::ROUTE;
        let caller = key(
            route.name,
            KeyObjectRef {
                kind: KeyTargetKind::Identity(KeyIdentityKind::System),
                id: None,
            },
            KeyAction::Read,
        );
        assert!(allowed(&route, actor(true), &caller, |_| None).unwrap());
        assert!(!caller.as_admin());
        assert!(!caller.carries_system_admin());
        assert!(
            !allowed(
                &orch_api_contract::admin::AdminStorageGet::ROUTE,
                actor(true),
                &caller,
                |_| None
            )
            .unwrap()
        );
        let mut withdrawn = actor(true);
        withdrawn.system_admin = false;
        assert!(!allowed(&route, withdrawn, &caller, |_| None).unwrap());
    }

    #[test]
    fn targeted_administration_does_not_require_an_artificial_system_target() {
        let route = orch_api_contract::admin::AdminOrgUpdate::ROUTE;
        let id = Id::from_u128(10);
        let caller = key(
            route.name,
            KeyObjectRef {
                kind: KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Org),
                id: Some(id),
            },
            KeyAction::Manage,
        );
        assert!(allowed(&route, actor(true), &caller, |_| Some(id)).unwrap());
        assert!(!allowed(&route, actor(true), &caller, |_| Some(Id::from_u128(11))).unwrap());
        assert!(!caller.carries_system_admin());
    }

    #[test]
    fn legacy_or_unadmitted_credentials_do_not_gain_admin_operations() {
        let route = orch_api_contract::admin::AdminSystemGet::ROUTE;
        let legacy = Caller::token(
            actor(true).id,
            TokenGrant::from_stored(&["*".into()], None)
                .unwrap()
                .for_token(Id::from_u128(2)),
        );
        assert!(!allowed(&route, actor(true), &legacy, |_| None).unwrap());
        let other_user = Caller::session(Id::from_u128(20)).with_admin_mode(true);
        assert!(!allowed(&route, actor(false), &other_user, |_| None).unwrap());
    }

    #[test]
    fn node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need() {
        use orch_access_contract::permissions::Permission;
        use orch_settings::validation::Need;
        let id = Id::from_u128(10);
        assert!(covers_node_settings(
            id,
            Need::OnNode {
                permission: Permission::SettingsManage,
                node: id
            }
        ));
        for need in [
            Need::OnNode {
                permission: Permission::SettingsManage,
                node: Id::from_u128(11),
            },
            Need::OnNode {
                permission: Permission::ThreadManage,
                node: id,
            },
            Need::OnEnvironment {
                permission: Permission::EnvironmentManage,
                environment: id,
            },
            Need::OnProvider {
                permission: Permission::ProviderManage,
                provider: id,
            },
            Need::SystemAdmin,
        ] {
            assert!(!covers_node_settings(id, need));
        }
    }
}
