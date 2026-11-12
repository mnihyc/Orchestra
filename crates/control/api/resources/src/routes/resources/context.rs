//! Native contextual projections of the existing resource read operations.
//!
//! A composition supplies its genuine operation Ctx and selected place. The resource
//! keeps its real home; no URL, model assertion or global Manage becomes a work grant.

use crate::key_operations::native;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::routes::nodes::view;

use orch_access_contract::key_scope::KeyAction;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

#[cfg(any(test, feature = "test-support"))]
use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::PageQuery;

use orch_api_contract::nodes::NodeRef;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;

use orch_api_contract::resources::RESOURCE_READ_CONTEXT;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::resources;

use orch_db::Conn;

use orch_types::Id;

pub use orch_api_base::routes::resources::context::Request;

pub use orch_api_base::routes::resources::context::options_json;

pub use orch_api_base::routes::resources::context::Resource;

pub use orch_api_base::routes::resources::context::Projection;

pub use orch_api_base::routes::resources::context::ContextFacts;

pub use orch_api_base::routes::resources::context::Prepared;
fn family(operation: &str) -> ApiResult<(ResourceKind, bool)> {
    match operation {
        "ProviderGet" => Ok((ResourceKind::Provider, true)),
        "ProviderList" => Ok((ResourceKind::Provider, false)),
        "AgentGet" => Ok((ResourceKind::Agent, true)),
        "AgentList" => Ok((ResourceKind::Agent, false)),
        _ => Err(ApiError::internal(
            "this operation has no contextual resource projection",
        )),
    }
}
orch_api_base::__api_bind_4961d39df5!();

fn index(plan: &OperationPlan) -> Option<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(RESOURCE_READ_CONTEXT))
}

/// Full native projections have not selected a work context. This is genuine absence.
pub(crate) fn empty(plan: &mut OperationPlan) -> ApiResult<()> {
    if let Some(index) = index(plan) {
        plan.bind_empty(index)?;
    }
    Ok(())
}

fn primary(plan: &OperationPlan, kind: ResourceKind) -> ApiResult<usize> {
    let kind = KeyTargetKind::Object(kind.into());
    plan.variant()
        .facets
        .iter()
        .position(|facet| {
            facet.usage == FacetUsage::Required
                && facet.action == KeyAction::Read
                && facet.resources == [kind]
                && (matches!(facet.target, TargetResolver::TypedPath { .. })
                    || facet.target == TargetResolver::ResolvedCollection("results"))
        })
        .ok_or_else(|| ApiError::internal("the resource read's actual subject is undeclared"))
}

/// Extract ONLY this producer's explicitly declared context role. Generic source names,
/// a provider's home and an arbitrary readable node do not establish a context share.
pub fn facts(plan: &OperationPlan) -> ApiResult<Option<ContextFacts>> {
    let Some(index) = index(plan) else {
        return Ok(None);
    };
    let (kind, _) = family(&plan.variant().operation)?;
    let context = plan
        .shared()
        .response_facets_for(&[index])
        .map_err(crate::key_operations::admission::binding_error)?;
    if context.is_empty() {
        return Ok(None);
    }
    if context.len() != 1
        || context[0].target.object.id.is_none()
        || context[0].target.object.kind != context[0].resource_kind
        || !matches!(
            context[0].resource_kind,
            KeyTargetKind::Object(
                ObjectKind::Org | ObjectKind::Project | ObjectKind::Topic | ObjectKind::Thread
            )
        )
    {
        return Err(ApiError::internal(
            "a contextual resource read needs one actual place",
        ));
    }
    let required = plan
        .shared()
        .required_facets()
        .map_err(crate::key_operations::admission::binding_error)?;
    let family = KeyTargetKind::Object(kind.into());
    if required
        .iter()
        .any(|facet| facet.resource_kind != family || facet.action != KeyAction::Read)
    {
        return Err(ApiError::internal(
            "a contextual resource projection has an unrelated required source",
        ));
    }
    if plan
        .shared()
        .response_facets()
        .map_err(crate::key_operations::admission::binding_error)?
        .len()
        != context.len()
    {
        return Err(ApiError::internal(
            "a limited contextual projection has unrelated response sources",
        ));
    }
    Ok(Some(ContextFacts {
        context: context[0].target.clone(),
        resources: required.into_iter().map(|facet| facet.target).collect(),
    }))
}
orch_api_base::__api_bind_35df7b5ec1!();

fn omitted_collection(target: TargetResolver) -> bool {
    matches!(
        target,
        TargetResolver::ResolvedCollection(
            "provider_groups"
                | "provider_names_orgs"
                | "provider_names_projects"
                | "provider_names_topics"
                | "provider_names_threads"
                | "agent_providers"
                | "agent_machines"
                | "agent_groups"
                | "agent_names_orgs"
                | "agent_names_projects"
                | "agent_names_topics"
                | "agent_names_threads"
        )
    )
}

fn bind_resolved(
    route: orch_api_contract::routes::Route,
    node: KeyResolvedTarget,
    resources: Vec<KeyResolvedTarget>,
) -> ApiResult<OperationPlan> {
    let (kind, _) = family(route.name)?;
    let family = KeyTargetKind::Object(kind.into());
    if resources
        .iter()
        .any(|target| target.object.kind != family || target.object.id.is_none())
        || node.object.id.is_none()
    {
        return Err(ApiError::internal(
            "a contextual read needs actual resource and place identities",
        ));
    }
    let mut plan = OperationPlan::new(route, route.name)?;
    let primary = primary(&plan, kind)?;
    let context =
        index(&plan).ok_or_else(|| ApiError::internal("the resource context role is missing"))?;
    for index in 0..plan.variant().facets.len() {
        if index == primary {
            if resources.is_empty() {
                plan.bind_empty(index)?;
            } else {
                plan.bind(index, family, &resources)?;
            }
        } else if index == context {
            plan.bind(index, node.object.kind, std::slice::from_ref(&node))?;
        } else if omitted_collection(plan.variant().facets[index].target) {
            // This limited projection has no ACL group, home path, machine, profile
            // association or other resource name in its output. None is supplied.
            plan.bind_empty(index)?;
        } else {
            return Err(ApiError::internal(
                "a contextual resource source was not produced",
            ));
        }
    }
    Ok(plan)
}

async fn bind(
    ctx: &Ctx,
    conn: &mut Conn,
    node: Id,
    grants: &[resources::context::Grant],
) -> ApiResult<(OperationPlan, ContextFacts)> {
    let (kind, _) = family(ctx.route.name)?;
    let node = targets::node(conn, node).await?;
    let mut resources = Vec::new();
    for grant in grants {
        resources.push(
            targets::page_object(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: kind.into(),
                    id: grant.resource.id,
                },
            )
            .await?,
        );
    }
    let plan = bind_resolved(ctx.route, node.clone(), resources.clone())?;
    Ok((
        plan,
        ContextFacts {
            context: node,
            resources,
        },
    ))
}
orch_api_base::__api_bind_19bde9b549!();

async fn node_ref(conn: &mut Conn, principal: &Principal, node: Id) -> ApiResult<NodeRef> {
    let mut refs = view::refs(conn, principal, &[node], Policy::default()).await?;
    let mut node = refs.remove(&node).ok_or_else(ApiError::not_found)?;
    // This source names the selected place; it does not read its conversation.
    node.thread_display = None;
    Ok(node)
}
orch_api_base::__api_bind_75a6668c1c!();

fn project(context: NodeRef, grants: &[resources::context::Grant], next: Option<Id>) -> Projection {
    Projection {
        projection: "context",
        context,
        next_cursor: next.map(|id| id.to_string()),
        items: grants
            .iter()
            .map(|grant| Resource {
                id: grant.resource.id,
                kind: grant.resource.kind.as_str().to_owned(),
                name: grant.resource.name.clone(),
                disabled: grant.resource.disabled,
                permission: grant.permission,
                options: grant.options.clone(),
            })
            .collect(),
    }
}

async fn grants_at(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    ids: &[Id],
) -> ApiResult<Vec<resources::context::Grant>> {
    match resources::context::among(conn, principal, node, ids, Policy::default()).await {
        Ok(grants) => Ok(grants),
        Err(error) => Err(authorization::refusals::problem(conn, error).await),
    }
}
orch_api_base::__api_bind_0f877a22b0!();

/// Select actual work grants before the window; global ownership or Manage is never a candidate grant.
/// The caller is the original/current admitted HTTP caller, including its key and assurance ceiling.
pub async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    request: Request,
) -> ApiResult<Prepared> {
    if !orch_db::queries::transaction_guard::in_transaction(conn) {
        return Err(ApiError::internal(
            "a native contextual read needs its own snapshot",
        ));
    }
    if !crate::key_operations::admission::same_response_origin(ctx, caller)? {
        return Err(refused());
    }
    let (kind, single) = family(ctx.route.name)?;
    if ctx.route.method != orch_api_contract::routes::Method::GET {
        return Err(ApiError::internal("a resource context is a native read"));
    }
    let known = OperationPlan::new(ctx.route, ctx.route.name)?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    // Establish the real node's native View even when the result is empty.
    grants_at(conn, &principal, request.node, &[]).await?;
    let primary = primary(&known, kind)?;
    let family = KeyTargetKind::Object(kind.into());
    let limit = usize::try_from(
        PageQuery {
            limit: request.limit,
            cursor: None,
        }
        .limit(),
    )
    .map_err(ApiError::internal)?;
    let mut grants = Vec::new();
    let mut cursor = request.after;
    let mut next = None;
    loop {
        let ids = if single {
            vec![ctx.id("id")?]
        } else {
            resources::candidates(conn, &principal, kind, cursor)
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .map(|row| row.id)
                .collect::<Vec<_>>()
        };
        if ids.is_empty() {
            break;
        }
        let last = *ids.last().ok_or_else(ApiError::not_found)?;
        if !single && cursor.is_some_and(|cursor| last <= cursor) {
            return Err(ApiError::internal(
                "the native resource cursor did not advance",
            ));
        }
        let found = grants_at(conn, &principal, request.node, &ids).await?;
        let mut found = found
            .into_iter()
            .filter(|grant| grant.resource.kind == kind)
            .collect::<Vec<_>>();
        found.sort_by_key(|grant| grant.resource.id);
        for grant in found {
            let target = targets::page_object(
                conn,
                orch_access_contract::decisions::ObjectRef {
                    kind: kind.into(),
                    id: grant.resource.id,
                },
            )
            .await?;
            if known.permits_binding(
                principal.caller(),
                primary,
                family,
                std::slice::from_ref(&target),
            )? {
                grants.push(grant);
                if !single && grants.len() > limit {
                    break;
                }
            }
        }
        if single || grants.len() > limit {
            break;
        }
        cursor = Some(last);
    }
    if single && grants.len() != 1 {
        return Err(ApiError::not_found());
    }
    if !single && grants.len() > limit {
        grants.truncate(limit);
        next = grants.last().map(|grant| grant.resource.id);
    }
    let context = node_ref(conn, &principal, request.node).await?;
    let projection = project(context, &grants, next);
    let (response, facts) = bind(ctx, conn, request.node, &grants).await?;
    let response = response.within_caller(ctx, principal.caller())?;
    let admitted = response
        .preparation_principal(ctx, conn, principal.caller())
        .await?;
    let origin = CapturedOperation::capture(admitted.caller()).map_err(ApiError::internal)?;
    Ok(Prepared {
        response,
        origin,
        request,
        grants,
        facts,
        projection,
    })
}
orch_api_base::__api_bind_b57f3f3ce1!();

fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the original contextual resource share is no longer readable",
    )
}
orch_api_base::__api_bind_e0bd87e4b3!();

#[cfg(any(test, feature = "test-support"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a deciding case fails by panicking"
)]
pub mod tests {

    use super::*;

    use orch_access_contract::key_scope::KeyObjectRef;

    use orch_access_contract::key_scope::KeyScope;

    use orch_access_contract::permissions::Placement;

    use orch_api_contract::resources::AgentGet;

    use orch_api_contract::resources::AgentList;

    use orch_api_contract::resources::ProviderGet;

    use orch_api_contract::resources::ProviderList;

    use orch_api_contract::routes::Endpoint;

    use orch_types::new_id;

    use serde_json::json;
    fn target(kind: ObjectKind, id: Id, home: Option<Id>) -> KeyResolvedTarget {
        KeyResolvedTarget {
            object: KeyObjectRef {
                kind: KeyTargetKind::Object(kind),
                id: Some(id),
            },
            node_ancestors: home
                .map(|id| KeyObjectRef {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(id),
                })
                .into_iter()
                .collect(),
            document_ancestors: vec![],
            credential_account: None,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn every_context_read_keeps_one_business_selection_and_both_real_independent_sources() {
        let root = new_id();
        let home = new_id();
        let resource = new_id();
        let node = new_id();
        for (route, kind) in [
            (ProviderGet::ROUTE, ObjectKind::Provider),
            (ProviderList::ROUTE, ObjectKind::Provider),
            (AgentGet::ROUTE, ObjectKind::Agent),
            (AgentList::ROUTE, ObjectKind::Agent),
        ] {
            let object = target(kind, resource, Some(home));
            let context = target(ObjectKind::Topic, node, Some(root));
            let plan = bind_resolved(route, context.clone(), vec![object.clone()]).unwrap();
            assert_eq!(plan.variant().operation, route.name);
            let sources = facts(&plan).unwrap().unwrap();
            assert_eq!(sources.context, context);
            assert_eq!(sources.resources, [object]);
            assert_ne!(
                sources.context.node_path(),
                sources.resources[0].node_path(),
                "a share does not relabel the provider/profile as a child of the selected place"
            );
            let mut facets = plan.shared().required_facets().unwrap();
            facets.extend(plan.shared().response_facets().unwrap());
            let resource_scope = json!({"target":{"kind":kind.as_str(),"id":resource,"reach":"self"},
                "allow":[{"operation":route.name,"actions":["read"]}]});
            let place_scope = json!({"target":{"kind":"topic","id":node,"reach":"self"},
                "allow":[{"operation":route.name,"actions":["read"]}]});
            let exact: KeyScope = serde_json::from_value(
                json!({"rules":[resource_scope.clone(),place_scope.clone()]}),
            )
            .unwrap();
            assert!(exact.allows(&facets));
            for rule in [resource_scope, place_scope] {
                let partial: KeyScope = serde_json::from_value(json!({"rules":[rule]})).unwrap();
                assert!(
                    !partial.allows(&facets),
                    "neither independent source supplies the other's Read"
                );
            }
            assert!(
                bind_resolved(
                    route,
                    context,
                    vec![target(ObjectKind::Topic, resource, Some(root))]
                )
                .is_err(),
                "a URL parent cannot replace the real governing resource"
            );
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_every_context_read_keeps_one_business_selection_and_both_real_independent_sources()
     {
        self::every_context_read_keeps_one_business_selection_and_both_real_independent_sources()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn an_ordinary_home_or_name_fact_is_not_a_context_share() {
        let mut plan = OperationPlan::new(ProviderGet::ROUTE, "ProviderGet").unwrap();
        empty(&mut plan).unwrap();
        let org = target(ObjectKind::Org, new_id(), None);
        let names = plan
            .variant()
            .facets
            .iter()
            .position(|facet| {
                facet.target == TargetResolver::ResolvedCollection("provider_names_orgs")
            })
            .unwrap();
        plan.bind(names, org.object.kind, &[org]).unwrap();
        assert!(facts(&plan).unwrap().is_none());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_ordinary_home_or_name_fact_is_not_a_context_share() {
        self::an_ordinary_home_or_name_fact_is_not_a_context_share()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
     {
        let private_home = new_id();
        let private_owner = new_id();
        let private_machine = new_id();
        let context = NodeRef {
            id: new_id(),
            kind: orch_api_contract::nodes::NodeKind::Topic,
            name: Some("Actual selected context".into()),
            redacted: false,
            thread_display: None,
        };
        for (kind, permission) in [
            (ResourceKind::Provider, Permission::ProviderUse),
            (ResourceKind::Agent, Permission::AgentUse),
        ] {
            let grant = resources::context::Grant {
                resource: resources::Resource {
                    kind,
                    id: new_id(),
                    name: "The resource shared here".into(),
                    first_owner: Some(private_owner),
                    home: Some(private_home),
                    root: Some(private_home),
                    disabled: false,
                    deleted: false,
                    placement: Placement::Managed,
                    subscription: None,
                    arrangements: vec![],
                    machine: Some(private_machine),
                    machine_owner: Some(private_owner),
                },
                permission,
                options: orch_authz::options::unlimited(permission),
                sources: vec![],
            };
            let body = serde_json::to_value(project(context.clone(), &[grant], None)).unwrap();
            assert_eq!(body["projection"], "context");
            assert_eq!(body["items"][0]["name"], "The resource shared here");
            for field in [
                "home",
                "org_id",
                "owner",
                "owners",
                "machine",
                "origin_machine",
                "profile",
                "descriptor",
                "runtime",
                "config",
                "facts",
                "usage",
                "choice_usage",
                "running_threads",
                "models",
                "providers",
            ] {
                assert!(
                    body["items"][0].get(field).is_none(),
                    "context data has no {field}"
                );
            }
            for id in [private_home, private_owner, private_machine] {
                assert!(!body.to_string().contains(&id.to_string()));
            }
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
     {
        self::the_actual_context_projection_never_serializes_home_owner_machine_profile_usage_or_other_work()
    }
}
