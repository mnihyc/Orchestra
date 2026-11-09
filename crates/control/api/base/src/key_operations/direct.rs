//! Bind audited direct-target operations through their explicit canonical metadata.
//!
//! This adapter contains no endpoint-name list and no URL/body/family inference. Ordinary
//! producers declare each real target kind and keep their existing guards and writers.
//! Compound variants, dynamic roles and untyped paths still require their own producer.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::operations;

use orch_api_contract::operations::Delegation;

use orch_api_contract::operations::OperationVariant;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::routes::Route;

use orch_authz::Caller;

use orch_db::Conn;

use super::targets;
fn unresolved() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the operation needs its typed target producer",
    )
}

fn declared_variant(route: &Route) -> ApiResult<OperationVariant> {
    let declaration = operations::for_route(route).map_err(ApiError::internal)?;
    if declaration.delegation != Delegation::Business {
        return Err(unresolved());
    }
    let [variant] = declaration.variants.as_slice() else {
        return Err(unresolved());
    };
    if !variant.name.is_empty() || variant.operation != route.name {
        return Err(unresolved());
    }
    for facet in &variant.facets {
        if !matches!(
            facet.target,
            TargetResolver::TypedPath { .. }
                | TargetResolver::PermissionObject
                | TargetResolver::CurrentAccount
                | TargetResolver::System
        ) {
            return Err(unresolved());
        }
        // Multiple families require the actual operation's typed selection. A creation
        // parent's kind is never substituted for the business family of its new object.
        if facet.resources.len() != 1 {
            return Err(unresolved());
        }
    }
    Ok(variant.clone())
}

async fn prepare(ctx: &Ctx, conn: &mut Conn) -> ApiResult<OperationPlan> {
    let variant = declared_variant(&ctx.route)?;
    let mut plan = OperationPlan::new(ctx.route, &variant.operation)?;
    let bindings = targets::TargetBindings::default();
    for (index, facet) in variant.facets.iter().enumerate() {
        let family = facet.resources.first().ok_or_else(unresolved)?;
        let resolved = targets::resolve(ctx, conn, facet, &bindings).await?;
        plan.bind(index, *family, &resolved)?;
    }
    Ok(plan)
}

/// Reused by the audited endpoint registrations; inputs are validated by their normal
/// contract but contain no additional target/effect selection for this cohort.
pub fn declared<'a, Q, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    _query: Q,
    _body: B,
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn))
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_access_contract::key_scope::KeyAction;

    use orch_access_contract::key_scope::KeyIdentityKind;

    use orch_access_contract::key_scope::KeyTargetKind;

    use orch_api_contract::operations::OperationSpec;

    use orch_api_contract::routes::Endpoint;
    #[cfg(any(test, feature = "test-support"))]
    fn untyped_paths_dynamic_sources_and_compound_variants_remain_closed() {
        assert!(declared_variant(&orch_api_contract::chats::ChatDraftSet::ROUTE).is_err());
        assert!(
            declared_variant(&orch_api_contract::document_tree::DocumentTreeOperate::ROUTE)
                .is_err()
        );
        assert!(declared_variant(&orch_api_contract::chats::NodeChatOpeningGet::ROUTE).is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_untyped_paths_dynamic_sources_and_compound_variants_remain_closed() {
        self::untyped_paths_dynamic_sources_and_compound_variants_remain_closed()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn empty_wire_reply_is_a_receipt_with_optional_error_disclosure() {
        let route = orch_api_contract::chats::ChatRead::ROUTE;
        let operation = operations::for_route(&route).unwrap();
        assert!(operation.receipt);
        let variant = declared_variant(&route).unwrap();
        assert_eq!(variant.facets.len(), 2);
        assert_eq!(variant.facets[0].action, KeyAction::Write);
        assert_eq!(variant.facets[0].usage, operations::FacetUsage::Required);
        assert_eq!(variant.facets[1].action, KeyAction::Read);
        assert_eq!(
            variant.facets[1].usage,
            operations::FacetUsage::ResponseContent
        );
        let mut plan = OperationPlan::new(route, route.name).unwrap();
        assert!(
            plan.bind_empty(0).is_err(),
            "typed identities remain singletons"
        );
        let mut raw = route;
        raw.reply = orch_api_contract::routes::Shape::Raw;
        assert!(!operations::for_route(&raw).unwrap().receipt);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_empty_wire_reply_is_a_receipt_with_optional_error_disclosure() {
        self::empty_wire_reply_is_a_receipt_with_optional_error_disclosure()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist() {
        let mut route = orch_api_contract::users::UserGet::ROUTE;
        route.operation = Some(OperationSpec::single(
            KeyAction::Manage,
            &[KeyTargetKind::Identity(KeyIdentityKind::Key)],
            TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            },
        ));
        let declaration = declared_variant(&route).unwrap();
        assert_eq!(
            declaration.facets[0].resources,
            [KeyTargetKind::Identity(KeyIdentityKind::Key)]
        );
        assert_eq!(
            declaration.facets[0].target,
            TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            }
        );
        route.operation = Some(OperationSpec::single(
            KeyAction::Read,
            &[KeyTargetKind::Identity(KeyIdentityKind::Account)],
            TargetResolver::TypedPath {
                parameter: "missing",
                kind: KeyTargetKind::Identity(KeyIdentityKind::Account),
            },
        ));
        assert!(operations::for_route(&route).is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist()
     {
        self::explicit_parent_kind_is_independent_of_business_family_and_parameter_must_exist()
    }
}
