//! Credential bounds for the existing role writers and their optional metadata projection.

use super::{Placed, roles_held, roles_held_bounded, the_role};
use crate::key_operations::{resolver::Disclosure, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use futures::future::BoxFuture;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access as api;
use orch_api_contract::common::ReceiptProjection;
use orch_authz::{Caller, Principal};
use orch_db::queries::access;
use orch_db::{Conn, Tx};
use orch_types::Id;
use std::future::Future;

const NAMES: NameRoles = NameRoles {
    groups: Some("role_groups"),
    nodes: [
        (ObjectKind::Org, "role_names_orgs"),
        (ObjectKind::Project, "role_names_projects"),
        (ObjectKind::Topic, "role_names_topics"),
        (ObjectKind::Thread, "role_names_threads"),
    ],
    containers: None,
};

/// The same stored role that supplied this exact guard's actual effect target.
struct PreparedRole {
    row: access::RoleRow,
    place: Placed,
}

async fn base(ctx: &Ctx, conn: &mut Conn, node: Id) -> ApiResult<OperationPlan> {
    let target = targets::node(conn, node).await?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    plan.bind(0, target.object.kind, std::slice::from_ref(&target))?;
    if ctx.route.name != "RoleDelete" {
        // The full reply remains unresolved until the original producer has its actual row.
        // A denied Read already stays denied; an unresolved Read is never treated as granted.
        plan.bind(1, target.object.kind, std::slice::from_ref(&target))?;
    }
    Ok(plan)
}

pub(super) fn create<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    body: api::RoleCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let node = ctx.id("id")?;
        super::clean_note(body.note.as_deref())?;
        super::expiry(body.expires_at, ctx.now)?;
        base(ctx, conn, node).await
    })
}

async fn existing(ctx: &Ctx, conn: &mut Conn, role: Id) -> ApiResult<OperationPlan> {
    let (row, place) = the_role(conn, role).await?;
    Ok(base(ctx, conn, row.node_id)
        .await?
        .with_data(PreparedRole { row, place }))
}

pub(super) fn update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    body: api::RoleUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        // Preserve the original handler's note-validation order for ordinary calls.
        if let Some(note) = body.note.as_deref() {
            super::clean_note(Some(note))?;
        }
        if let Some(Some(at)) = body.expires_at {
            super::expiry(Some(at), ctx.now)?;
        }
        existing(ctx, conn, id).await
    })
}

pub(super) fn delete<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move { existing(ctx, conn, ctx.id("id")?).await })
}

/// Reuse the existing identity/credential guard, retaining its own operands rather than
/// another context clone's earlier preparation. Current audit identity/time are unchanged.
pub(super) async fn action(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Guard)> {
    let (tx, guard) = transaction_guard::begin(ctx).await?;
    let current = ctx
        .for_operation_checkpoint(guard.user().clone(), guard.now().get())
        .with_admitted_caller(guard.principal().caller().clone())?;
    Ok((current, tx, guard))
}

pub(super) fn guarded_role(guard: &Guard) -> ApiResult<(access::RoleRow, Placed)> {
    let prepared = guard.operation_data::<PreparedRole>()?;
    Ok((prepared.row.clone(), prepared.place))
}

/// Preserve the ordinary refusal's code/status without constructing a second error model.
/// An empty delete response delegates no content disclosure, including on a refusal.
pub(super) fn projected<'a, T: 'a>(
    ctx: Ctx,
    work: impl Future<Output = ApiResult<T>> + Send + 'a,
) -> BoxFuture<'a, ApiResult<T>> {
    let content_free = ctx.route.name == "RoleDelete";
    authorization::projected_result(ctx, work, content_free)
}

fn sources(row: &access::RoleRow) -> NameSources {
    let mut sources = NameSources::default();
    sources.nodes.insert(row.node_id);
    sources.users.insert(row.created_by);
    match row.subject_kind.as_str() {
        "user" => {
            sources.users.insert(row.subject_id);
        }
        "group" => {
            sources.groups.insert(row.subject_id);
        }
        "role" | "members" => {
            sources.nodes.insert(row.subject_id);
        }
        _ => {}
    }
    sources
}

/// The ordinary role metadata is projected only after this operation's actual source Read.
/// Missing optional content keeps the accepted write and its minimal acknowledgement.
pub(super) async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &access::RoleRow,
    root: Id,
) -> ApiResult<Option<api::RoleHeld>> {
    if principal
        .caller()
        .grant()
        .is_none_or(|grant| grant.operation_scope().is_none())
    {
        return roles_held(conn, principal, std::slice::from_ref(row), root)
            .await?
            .into_iter()
            .next()
            .map(Some)
            .ok_or_else(ApiError::not_found);
    }
    if ctx.key_receipt_required() && ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    // Repeat current placement on the writer's connection. No old path or nearby role id
    // supplies the response's Read, even when a preceding request used the same role.
    let result = async {
        let mut plan = base(ctx, conn, row.node_id).await?;
        let bounds: NameBounds =
            projection_names(conn, principal.caller(), &mut plan, sources(row), &NAMES).await?;
        let row = row.clone();
        plan.read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                roles_held_bounded(conn, reader, &[row], root, Some(&bounds))
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(ApiError::not_found)
            })
        })
        .await
    }
    .await;
    match result {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

pub(super) fn receipt(row: &access::RoleRow) -> api::RoleWriteResponse {
    api::RoleWriteResponse::Receipt(api::RoleWriteReceipt {
        projection: ReceiptProjection::Receipt,
        role_id: row.id,
        node_id: row.node_id,
        revision: row.revision,
    })
}

/// Read the just-created role for the same shared mutation producer, before projection.
pub(super) async fn created(conn: &mut Conn, id: Id) -> ApiResult<access::RoleRow> {
    access::role_row(conn, id)
        .await?
        .ok_or_else(|| ApiError::internal("a role was given and is not there"))
}
