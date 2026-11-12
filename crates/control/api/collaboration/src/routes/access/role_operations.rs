//! Credential bounds for the existing role writers and their optional metadata projection.

use super::Placed;

use super::roles_held;

use super::roles_held_bounded;

use super::the_role;

use crate::key_operations::resolver::Disclosure;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::Guard;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use futures::future::BoxFuture;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::access as api;

use orch_api_contract::common::ReceiptProjection;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::queries::access;

use orch_db::Conn;

use orch_db::Tx;

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

pub(super) struct PreparedCreation {
    pub disclosure: crate::thread_read_tools::sharing::Before,
}

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
        let plan = base(ctx, conn, node).await?;
        let disclosure = crate::thread_read_tools::sharing::Before::take(
            &ctx.app,
            conn,
            Some(node),
            &("role", Some(node), &body),
            false,
            true,
        )
        .await?;
        Ok(plan.with_data(PreparedCreation { disclosure }))
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
    read_mode(ctx, conn, principal, row, root, false).await
}

async fn read_mode(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &access::RoleRow,
    root: Id,
    replaying: bool,
) -> ApiResult<Option<api::RoleHeld>> {
    if principal
        .caller()
        .grant()
        .is_none_or(|grant| grant.operation_scope().is_none())
    {
        if replaying {
            authorization::require(
                conn,
                principal,
                row.node_id,
                authorization::VIEW,
                orch_authz::Act::Use,
                orch_authz::Policy::default(),
            )
            .await?;
        }
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
        let projection = if replaying {
            crate::key_operations::admission::ResponseProjection::Replay(
                plan.variant()
                    .facets
                    .iter()
                    .enumerate()
                    .filter_map(|(index, facet)| {
                        (facet.usage == orch_api_contract::operations::FacetUsage::ResponseContent)
                            .then_some(index)
                    })
                    .collect(),
            )
        } else {
            crate::key_operations::admission::ResponseProjection::CompletedContent
        };
        plan.read_response_projection(
            ctx,
            conn,
            principal.caller(),
            projection,
            move |conn, reader| {
                Box::pin(async move {
                    if replaying {
                        authorization::require(
                            conn,
                            reader,
                            row.node_id,
                            authorization::VIEW,
                            orch_authz::Act::Use,
                            orch_authz::Policy::default(),
                        )
                        .await?;
                    }
                    roles_held_bounded(conn, reader, &[row], root, Some(&bounds))
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(ApiError::not_found)
                })
            },
        )
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

pub(super) fn replay_binding()
-> crate::middleware::idempotency::Creation<(), api::RoleCreateBody, api::RoleWriteResponse> {
    crate::middleware::idempotency::Creation {
        kind: "role",
        replay,
        identity: |value| {
            let (id, node, revision) = match value {
                api::RoleWriteResponse::Full(value) => (value.id, value.node.id, value.revision),
                api::RoleWriteResponse::Receipt(value) => {
                    (value.role_id, value.node_id, value.revision)
                }
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "role".into(),
                id,
                data: serde_json::json!({"node":node,"revision":revision}),
            })
        },
    }
}

fn replay(
    ctx: Ctx,
    (): (),
    _body: api::RoleCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<api::RoleWriteResponse>> {
    Box::pin(async move {
        let node = serde_json::from_value(
            created
                .data
                .get("node")
                .cloned()
                .ok_or_else(|| ApiError::internal("the accepted role has no native node"))?,
        )
        .map_err(ApiError::internal)?;
        let revision =
            serde_json::from_value(
                created.data.get("revision").cloned().ok_or_else(|| {
                    ApiError::internal("the accepted role has no native revision")
                })?,
            )
            .map_err(ApiError::internal)?;
        let receipt = api::RoleWriteResponse::Receipt(api::RoleWriteReceipt {
            projection: ReceiptProjection::Receipt,
            role_id: created.id,
            node_id: node,
            revision,
        });
        crate::key_operations::publication::receipt(&ctx, &receipt)?;
        let ctx = ctx.for_completed_replay_projection();
        let result = async {
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let identity = transaction_guard::current_http_identity(
                &ctx,
                &mut tx,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            let principal =
                authorization::load_principal(&mut tx, identity.caller.clone(), identity.now)
                    .await?;
            let principal =
                crate::sign_in::second_step::hold(&mut tx, &identity.user, principal).await?;
            let (row, place) = the_role(&mut tx, created.id).await?;
            let full = read_mode(&ctx, &mut tx, &principal, &row, place.org, true).await?;
            tx.commit().await.map_err(orch_db::DbError::from)?;
            identity.check_deadline(&ctx.app)?;
            Ok::<_, ApiError>(full)
        }
        .await;
        match result {
            Ok(Some(value)) => Ok(api::RoleWriteResponse::Full(Box::new(value))),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(receipt),
            Err(error) => Err(error),
        }
    })
}
