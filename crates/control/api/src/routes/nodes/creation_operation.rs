//! Scope facts for the existing project and topic creation producers.
//!
//! The required Write names the actual parent. Optional content names the node
//! actually made; neither its identity nor its ancestry is predicted from a key.

use crate::key_operations::{admission::Prepared, resolver::Disclosure, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OptionalOperationFuture};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::nodes::{
    Node, NodeCreateReceipt, NodeKind, ProjectCreateBody, TopicCreateBody,
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::Conn;
use orch_types::Id;
use std::collections::BTreeSet;

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    parent_kind: ObjectKind,
    created_kind: ObjectKind,
    selection: &str,
) -> ApiResult<Prepared> {
    let parent = targets::permission_object(ctx, conn).await?;
    if parent.object.kind != KeyTargetKind::Object(parent_kind) {
        return Err(ApiError::not_found());
    }
    let mut plan = Prepared::new(ctx.route, selection)?;
    plan.bind(0, KeyTargetKind::Object(created_kind), &[parent])?;
    Ok(plan)
}

pub(super) fn project<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    _body: ProjectCreateBody,
) -> OperationFuture<'a> {
    Box::pin(prepare(
        ctx,
        conn,
        ObjectKind::Org,
        ObjectKind::Project,
        "ProjectCreate",
    ))
}

pub(super) fn project_child<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    _body: ProjectCreateBody,
) -> OperationFuture<'a> {
    Box::pin(prepare(
        ctx,
        conn,
        ObjectKind::Project,
        ObjectKind::Project,
        "ProjectChildCreate",
    ))
}

pub(super) fn topic<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: TopicCreateBody,
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if body.first_thread.is_some() {
            if caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                return Prepared::new(ctx.route, "TopicCreate.with_thread")?
                    .topic_creation_prefix()
                    .map(Some);
            }
            return Ok(None);
        }
        prepare(
            ctx,
            conn,
            ObjectKind::Project,
            ObjectKind::Topic,
            "TopicCreate.create",
        )
        .await
        .map(Some)
    })
}

pub(super) fn receipt(node_id: Id, kind: NodeKind) -> NodeCreateReceipt {
    NodeCreateReceipt {
        projection: ReceiptProjection::Receipt,
        node_id,
        kind,
    }
}

/// Complete the optional Read before invoking the normal node view. New nodes
/// have no inherited content grant: ancestor names use their own declared facets.
pub(super) async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    parent_kind: ObjectKind,
    created_kind: ObjectKind,
    selection: &str,
) -> ApiResult<Option<Node>> {
    if principal
        .caller()
        .grant()
        .is_none_or(|grant| grant.operation_scope().is_none())
    {
        return super::shown(ctx, conn, principal, id).await.map(Some);
    }
    let mut plan = prepare(ctx, conn, parent_kind, created_kind, selection).await?;
    let created = targets::node(conn, id).await?;
    if created.object.kind != KeyTargetKind::Object(created_kind) {
        return Err(ApiError::internal(
            "the creation returned another kind of node",
        ));
    }
    plan.bind(1, created.object.kind, std::slice::from_ref(&created))?;
    let mut allowed = BTreeSet::from([id]);
    for (slot, kind) in [(2, ObjectKind::Org), (3, ObjectKind::Project)] {
        let mut named = Vec::new();
        for ancestor in &created.node_ancestors {
            if ancestor.kind != KeyTargetKind::Object(kind) || ancestor.id == Some(id) {
                continue;
            }
            let ancestor_id = ancestor.id.ok_or_else(ApiError::not_found)?;
            let target = targets::node(conn, ancestor_id).await?;
            if plan.permits_binding(
                principal.caller(),
                slot,
                ancestor.kind,
                std::slice::from_ref(&target),
            )? {
                allowed.insert(ancestor_id);
                named.push(target);
            }
        }
        if named.is_empty() {
            plan.bind_empty(slot)?;
        } else {
            plan.bind(slot, KeyTargetKind::Object(kind), &named)?;
        }
    }
    let mut topic_sources = None;
    if created_kind == ObjectKind::Topic {
        let sources =
            crate::routes::environments::reading::topic_summary_sources(conn, principal, &[id])
                .await?;
        let mut environments = Vec::new();
        for &(_, environment) in &sources.bindings {
            environments.push(
                targets::page_object(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: environment,
                    },
                )
                .await?,
            );
        }
        if environments.is_empty() {
            plan.bind_empty(4)?;
        } else {
            plan.bind(
                4,
                KeyTargetKind::Object(ObjectKind::Environment),
                &environments,
            )?;
        }
        topic_sources = Some(sources);
    }
    let now = ctx.now;
    let result = plan
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                authorization::require(
                    conn,
                    reader,
                    id,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
                super::view::one_with_name_sources(
                    conn,
                    reader,
                    id,
                    now,
                    Policy::default(),
                    &allowed,
                    topic_sources.as_ref(),
                )
                .await?
                .ok_or_else(ApiError::not_found)
            })
        })
        .await;
    match result {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}
