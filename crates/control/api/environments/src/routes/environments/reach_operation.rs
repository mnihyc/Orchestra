//! Typed current sources of the two existing reach readers, using the shared operation binder.

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;

use orch_authz::Caller;

use orch_authz::engine::Graph;

use orch_authz::resources;

use orch_db::Conn;

use orch_types::Id;

use std::collections::BTreeSet;
pub(super) const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(super) const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

pub(super) const MOUNT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Mount);

pub(super) fn error(caller: &Caller, error: ApiError) -> ApiError {
    if caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

pub(super) async fn root(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<OperationPlan> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: ctx.id("id")?,
        },
    )
    .await
    .map_err(|problem| error(caller, problem))?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, ENVIRONMENT, &[target])?;
    Ok(plan)
}

fn slot(plan: &OperationPlan, role: &'static str, kind: KeyTargetKind) -> ApiResult<usize> {
    let mut slots = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.target == TargetResolver::ResolvedCollection(role));
    let Some((index, facet)) = slots.next() else {
        return Err(ApiError::internal(
            "the reach source collection is not declared",
        ));
    };
    if slots.next().is_some()
        || facet.usage != FacetUsage::Required
        || facet.resources != [kind]
        || facet.action != orch_access_contract::key_scope::KeyAction::Read
    {
        return Err(ApiError::internal(
            "the reach source collection metadata changed",
        ));
    }
    Ok(index)
}

/// Ordinary source selection happens first. This predicate only bounds the credential
/// for the actual stored Thread identities; it grants no account View or Environment Use.
pub(super) async fn threads(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    role: &'static str,
    ids: &BTreeSet<Id>,
) -> ApiResult<BTreeSet<Id>> {
    let index = slot(plan, role, THREAD)?;
    let graph = Graph::load(conn, &ids.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let mut selected = BTreeSet::new();
    let mut targets = Vec::new();
    for id in ids {
        let Some(site) = graph.site_of(*id) else {
            continue;
        };
        if site.kind != ObjectKind::Thread {
            return Err(ApiError::not_found());
        }
        let target = targets::from_site(&graph, &site)?;
        if plan.permits_binding(caller, index, THREAD, std::slice::from_ref(&target))? {
            selected.insert(*id);
            targets.push(target);
        }
    }
    if targets.is_empty() {
        plan.bind_empty(index)?;
    } else {
        plan.bind(index, THREAD, &targets)?;
    }
    Ok(selected)
}

/// Mount attachment content has its Mount's own current placement, never the Environment's
/// home or the physical location's Machine as substitute. Shared resource/site facts are batched.
pub(super) async fn mounts(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    role: &'static str,
    ids: &BTreeSet<Id>,
) -> ApiResult<BTreeSet<Id>> {
    let index = slot(plan, role, MOUNT)?;
    let mut graph = Graph::default();
    let rows = resources::read_into(conn, &mut graph, &ids.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let mut selected = BTreeSet::new();
    let mut targets = Vec::new();
    for row in rows.into_iter().filter(|row| {
        !row.deleted && row.object().kind == ObjectKind::Mount && ids.contains(&row.id)
    }) {
        let Some(site) = resources::site_of(&graph, &row) else {
            continue;
        };
        let target = targets::from_site(&graph, &site)?;
        if plan.permits_binding(caller, index, MOUNT, std::slice::from_ref(&target))? {
            selected.insert(row.id);
            targets.push(target);
        }
    }
    if targets.is_empty() {
        plan.bind_empty(index)?;
    } else {
        plan.bind(index, MOUNT, &targets)?;
    }
    Ok(selected)
}
