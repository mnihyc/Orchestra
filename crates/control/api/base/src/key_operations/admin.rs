//! Ordinary administrative operation admission, separate from browser private-access override.

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyFacet;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::operations;

use orch_api_contract::operations::Delegation;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::OperationFacet;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::routes::Route;

use orch_authz::Caller;

use orch_types::Id;

use crate::middleware::authentication::Authed;

use crate::middleware::authentication::Via;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;
/// A node configuration effect of this exact ordinary administrative operation.
/// Only the checked constructor can make it, and it borrows the current principal.
pub struct AdministrativeNode<'a> {
    principal: &'a orch_authz::Principal,
    node: Id,
}

impl AdministrativeNode<'_> {
    pub fn principal(&self) -> &orch_authz::Principal {
        self.principal
    }
    pub fn node(&self) -> Id {
        self.node
    }

    /// Only SettingsManage at this actual node is covered. Provider/environment
    /// needs, private reads and system-wide locks retain their existing checks.
    pub fn covers(&self, need: orch_settings::validation::Need) -> bool {
        covers_node_settings(self.node, need)
    }
}

fn covers_node_settings(target: Id, need: orch_settings::validation::Need) -> bool {
    matches!(need, orch_settings::validation::Need::OnNode { permission: orch_access_contract::permissions::Permission::SettingsManage, node } if node == target)
}

#[derive(Clone, Copy)]
enum Origin {
    OwnerRead,
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
                Via::OwnerRead { .. } => Origin::OwnerRead,
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
        Origin::OwnerRead => return Ok(false),
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
    let Some(facets) = caller.admitted_facets() else {
        return Ok(false);
    };
    if !facets.is_empty() && !caller.permits_facets(facets) {
        return Ok(false);
    }
    let operation = operations::for_route(route).map_err(ApiError::internal)?;
    if operation.delegation != Delegation::Business {
        return Ok(false);
    }
    let Some(variant) = operation
        .variants
        .iter()
        .find(|variant| Some(variant.operation.as_str()) == caller.admitted_operation())
    else {
        return Ok(false);
    };
    if !facets.iter().all(|facet| {
        facet.operation == variant.operation
            && variant
                .facets
                .iter()
                .any(|metadata| matches_facet(metadata, facet, actor.id, &path))
    }) {
        return Ok(false);
    }
    let required = variant
        .facets
        .iter()
        .filter(|metadata| metadata.usage == FacetUsage::Required)
        .collect::<Vec<_>>();
    if let Some(roles) = caller.admitted_required_roles() {
        if roles.iter().all(|role| role.facets.is_empty())
            && !super::admission::checked_empty_for(route, caller)?
        {
            return Ok(false);
        }
        // A flattened target can match several metadata roles. Only the builder's
        // complete role proof distinguishes a real empty collection from a missing one.
        return Ok(!required.is_empty()
            && required.len() == roles.len()
            && required.iter().zip(roles).all(|(metadata, role)| {
                role.action == metadata.action
                    && role.resources == metadata.resources
                    && if role.facets.is_empty() {
                        matches!(metadata.target, TargetResolver::ResolvedCollection(_))
                    } else {
                        role.facets
                            .iter()
                            .all(|facet| matches_facet(metadata, facet, actor.id, &path))
                    }
            }));
    }
    if required
        .iter()
        .any(|metadata| matches!(metadata.target, TargetResolver::ResolvedCollection(_)))
    {
        return super::admission::checked_empty_for(route, caller);
    }
    Ok(!facets.is_empty()
        && required.iter().all(|metadata| {
            facets
                .iter()
                .any(|facet| matches_facet(metadata, facet, actor.id, &path))
        }))
}

/// The same ordinary administration predicate for an already restored, currently
/// admitted issuer. This returns no caller or native private-administration mode.
pub fn accepted_system_administration(
    principal: &orch_authz::Principal,
    route: &Route,
) -> ApiResult<bool> {
    let caller = principal.caller();
    if caller.owner_read_source().is_some() {
        return Ok(false);
    }
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
    /// An ordinary administrative act at this actual native Node. Keys keep
    /// their exact operation/Manage target and current native Admin standing;
    /// this never lends browser private-access mode or a page OrgManage grant.
    pub async fn ordinary_node_administration(
        &self,
        conn: &mut orch_db::Conn,
        principal: &orch_authz::Principal,
        node: Id,
    ) -> ApiResult<()> {
        let caller = principal.caller();
        if !authorization::same_credential_origin(self.user()?, caller) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the native administration origin is not current",
            ));
        }
        let graph = orch_authz::engine::Graph::load(conn, &[node])
            .await
            .map_err(ApiError::internal)?;
        let site = graph.site_of(node).ok_or_else(ApiError::not_found)?;
        let kind = KeyTargetKind::Object(site.kind);
        let frame = orch_authz::engine::Frame::load(
            conn,
            principal,
            orch_authz::Policy::default(),
            graph,
            std::slice::from_ref(&site),
            &[],
        )
        .await
        .map_err(ApiError::internal)?;
        let allowed = if let Some(grant) = caller.grant() {
            let operation = operations::for_route(&self.route).map_err(ApiError::internal)?;
            let variant = operation
                .variants
                .iter()
                .find(|variant| Some(variant.operation.as_str()) == caller.admitted_operation());
            let facets = caller.admitted_facets().unwrap_or(&[]);
            grant.operation_scope().is_some()
                && !grant.requires_legacy_compatibility()
                && !caller.as_admin()
                && operation.delegation == Delegation::Business
                && caller.permits_facets(facets)
                && variant.is_some_and(|variant| {
                    facets.iter().any(|actual| {
                        actual.operation == variant.operation
                            && actual.action == KeyAction::Manage
                            && actual.resource_kind == kind
                            && actual.target.object.kind == kind
                            && actual.target.object.id == Some(node)
                            && variant.facets.iter().any(|metadata| {
                                metadata.usage == FacetUsage::Required
                                    && metadata.action == KeyAction::Manage
                                    && matches_facet(metadata, actual, principal.user(), &|name| {
                                        self.id(name).ok()
                                    })
                            })
                    })
                })
                && frame.administration_standing(&site)
        } else {
            frame.administers(&site) && caller.reaches(site.path.as_deref())
        };
        if !allowed || !caller.key_deadline_allows(self.app.clock.as_ref()) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "current native administration and this Node's scoped act are required",
            ));
        }
        Ok(())
    }

    /// Entry to an existing ordinary admin producer. Browser behavior keeps its explicit
    /// mode; an operation-scoped key needs exact current admission and current standing.
    /// This method never changes the caller's global/private administration predicates.
    pub fn ordinary_system_admin(&self) -> ApiResult<Id> {
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
    pub fn ordinary_system_admin_with(&self, user: &Authed, caller: &Caller) -> ApiResult<Id> {
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
    pub fn administrative_node<'a>(
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

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, reason = "test assertions fail by panicking")]
pub mod tests {

    use super::*;

    use orch_access_contract::key_scope::KeyAction;

    use orch_access_contract::key_scope::KeyObjectRef;

    use orch_access_contract::key_scope::KeyOperation;

    use orch_access_contract::key_scope::KeyReach;

    use orch_access_contract::key_scope::KeyResolvedTarget;

    use orch_access_contract::key_scope::KeyRule;

    use orch_access_contract::key_scope::KeyScope;

    use orch_access_contract::key_scope::KeyTarget;

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

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_browser_mode_and_current_standing_are_preserved() {
        self::browser_mode_and_current_standing_are_preserved()
    }

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_exact_system_read_is_admitted_without_private_override() {
        self::exact_system_read_is_admitted_without_private_override()
    }

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_targeted_administration_does_not_require_an_artificial_system_target() {
        self::targeted_administration_does_not_require_an_artificial_system_target()
    }

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_legacy_or_unadmitted_credentials_do_not_gain_admin_operations() {
        self::legacy_or_unadmitted_credentials_do_not_gain_admin_operations()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission() {
        use orch_access_contract::permissions::ObjectKind;
        use orch_api_contract::operations::{FacetSpec, OperationSpec};
        const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);
        const SPEC: OperationSpec = OperationSpec::facets(&[
            FacetSpec::required(
                KeyAction::Read,
                &[ORG],
                TargetResolver::ResolvedCollection("orgs"),
            ),
            FacetSpec::response_content(&[ORG], TargetResolver::ResolvedCollection("names")),
        ]);
        let mut route = orch_api_contract::admin::AdminOrgList::ROUTE;
        route.operation = Some(SPEC);
        let original = key(
            route.name,
            KeyObjectRef {
                kind: ORG,
                id: Some(Id::from_u128(10)),
            },
            KeyAction::Read,
        );
        let original = original.intersect_restrictions(&original).unwrap();
        for names in [false, true] {
            let mut plan = orch_engine::operation_scope::Prepared::new(route, route.name).unwrap();
            assert!(
                plan.admit(&original, false).is_err(),
                "missing is not selected empty"
            );
            plan.bind_empty(0).unwrap();
            if names {
                plan.bind_collection(
                    1,
                    &[(
                        ORG,
                        vec![KeyResolvedTarget {
                            object: KeyObjectRef {
                                kind: ORG,
                                id: Some(Id::from_u128(10)),
                            },
                            node_ancestors: Vec::new(),
                            document_ancestors: Vec::new(),
                            credential_account: None,
                        }],
                    )],
                )
                .unwrap();
            } else {
                plan.bind_empty(1).unwrap();
            }
            let admitted = plan.admit(&original, true).unwrap();
            let wrong_action = key(
                route.name,
                KeyObjectRef {
                    kind: ORG,
                    id: Some(Id::from_u128(10)),
                },
                KeyAction::Manage,
            );
            assert!(plan.admit(&wrong_action, true).is_err());
            assert!(allowed(&route, actor(true), &admitted, |_| None).unwrap());
            assert!(admitted.admitted_empty_facets().is_some());
            assert_eq!(
                admitted.admitted_facets().unwrap().len(),
                usize::from(names)
            );
            let current = authorization::readmit_current(original.clone(), &admitted).unwrap();
            assert_eq!(current.admitted_facets(), admitted.admitted_facets());
            assert_eq!(
                current.admitted_required_roles(),
                admitted.admitted_required_roles()
            );
            assert!(allowed(&route, actor(true), &current, |_| None).unwrap());
            assert!(!current.carries_system_admin());
            assert!(!current.as_admin());
            let mut withdrawn = actor(true);
            withdrawn.system_admin = false;
            assert!(!allowed(&route, withdrawn, &current, |_| None).unwrap());
            let mut other_key = actor(true);
            other_key.origin = Origin::Key {
                id: Id::from_u128(3),
            };
            assert!(!allowed(&route, other_key, &current, |_| None).unwrap());
            let mut other = route;
            other.name = "AnotherNativeRead";
            assert!(!allowed(&other, actor(true), &current, |_| None).unwrap());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission()
     {
        self::administration_accepts_real_empty_roles_and_keeps_optional_read_on_current_readmission(
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    fn mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target() {
        use orch_access_contract::permissions::ObjectKind;
        use orch_api_contract::operations::{FacetSpec, OperationSpec};
        const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);
        const SPEC: OperationSpec = OperationSpec::single(
            KeyAction::Manage,
            &[ORG],
            TargetResolver::TypedPath {
                parameter: "id",
                kind: ORG,
            },
        );
        const MIXED: OperationSpec = OperationSpec::facets(&[
            FacetSpec::required(
                KeyAction::Manage,
                &[ORG],
                TargetResolver::TypedPath {
                    parameter: "id",
                    kind: ORG,
                },
            ),
            FacetSpec::required(
                KeyAction::Manage,
                &[ORG],
                TargetResolver::ResolvedCollection("children"),
            ),
        ]);
        let mut route = orch_api_contract::admin::AdminOrgUpdate::ROUTE;
        route.operation = Some(MIXED);
        let root = Id::from_u128(10);
        let target = KeyResolvedTarget {
            object: KeyObjectRef {
                kind: ORG,
                id: Some(root),
            },
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        };
        let original = key(route.name, target.object, KeyAction::Manage);
        let original = original.intersect_restrictions(&original).unwrap();
        let mut plan = orch_engine::operation_scope::Prepared::new(route, route.name).unwrap();
        plan.bind(0, ORG, &[target]).unwrap();
        let partial = plan.preparation_caller(&original).unwrap();
        assert!(partial.admitted_required_roles().is_none());
        assert!(!allowed(&route, actor(true), &partial, |_| Some(root)).unwrap());
        assert!(plan.admit(&original, false).is_err());
        plan.bind_empty(1).unwrap();
        let admitted = plan.admit(&original, false).unwrap();
        assert!(allowed(&route, actor(true), &admitted, |_| Some(root)).unwrap());
        assert!(!allowed(&route, actor(true), &admitted, |_| Some(Id::from_u128(11))).unwrap());
        let flat = original
            .admit_facets(admitted.admitted_facets().unwrap())
            .unwrap()
            .into_caller();
        assert!(!allowed(&route, actor(true), &flat, |_| Some(root)).unwrap());
        let stripped = admitted.intersect_restrictions(&admitted).unwrap();
        assert!(stripped.admitted_required_roles().is_none());
        assert!(!allowed(&route, actor(true), &stripped, |_| Some(root)).unwrap());
        let current = authorization::readmit_current(stripped, &admitted).unwrap();
        assert!(allowed(&route, actor(true), &current, |_| Some(root)).unwrap());
        let narrowed = key(
            "AnotherNativeAct",
            KeyObjectRef {
                kind: ORG,
                id: Some(root),
            },
            KeyAction::Manage,
        );
        assert!(authorization::readmit_current(narrowed, &admitted).is_err());
        let mut changed = route;
        changed.operation = Some(SPEC);
        assert!(!allowed(&changed, actor(true), &admitted, |_| Some(root)).unwrap());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target()
     {
        self::mixed_empty_administration_requires_each_canonical_role_not_a_matching_flat_target()
    }

    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need()
     {
        self::node_settings_proof_never_covers_another_node_or_an_environment_provider_or_private_need()
    }
}
