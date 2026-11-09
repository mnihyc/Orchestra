//! Delegated native role selection, retaining its origin through acknowledgement and reconnect.

use super::*;
use crate::key_operations::native::{self, Stored};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_api_contract::{
    common::ReceiptProjection,
    machine_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_engine::commands::{
    CommandAuthority, MachineRolesAuthority, Outgoing, role_change_refusal,
};
use orch_link_contract::commands::MachineRolesSet as NativeRolesSet;

pub(super) struct Changes;
impl Stored for Changes {
    type Source = Source;
    type Value = mutations::Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::ROLES];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        <mutations::Machines as Stored>::load(ctx, conn, declaration)
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <mutations::Machines as Stored>::require(ctx, conn, principal, value)
    }
}
struct Content;
impl acknowledgement::Body for Content {
    type Reply = Machine;
    const DECLARATION: &'static Declaration<Source> = &sources::ROLES;
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _row: &'a MachineRow,
        machine: Option<Machine>,
    ) -> BoxFuture<'a, ApiResult<Machine>> {
        Box::pin(async move {
            machine.ok_or_else(|| {
                ApiError::internal("the native role acknowledgement has no individual Machine")
            })
        })
    }
}

pub(super) fn register(registry: &mut Registry) {
    registry.on_with_optional_operation(
        api::MachineRolesSet,
        native::stored_deferred::<Changes, _, _>,
        change,
    );
}

async fn change(
    ctx: Ctx,
    (): (),
    body: api::MachineRolesSetBody,
) -> ApiResult<api::MachineRolesResponse> {
    let projection = ctx.clone();
    match change_on(ctx, body).await {
        Ok(answer) => Ok(answer),
        Err(error) => Err(acknowledgement::refusal::<Content>(&projection, error).await?),
    }
}

async fn change_on(
    ctx: Ctx,
    body: api::MachineRolesSetBody,
) -> ApiResult<api::MachineRolesResponse> {
    let roles = orch_link_contract::enrollment_choices::connector_roles(&body.roles.join(","))
        .map_err(|detail| ApiError::invalid("roles", detail))?;
    if roles.len() != body.roles.len() {
        return Err(ApiError::invalid(
            "roles",
            "Choose each role as a separate name.",
        ));
    }
    let id = ctx.id("id")?;
    let (credential, raw_until) = transaction_guard::native_origin(&ctx).await?;
    let (mut conn, preliminary) = transaction_guard::begin(&ctx).await?;
    own_machine(&ctx, &mut conn, id).await?;
    require_machine(
        &mut conn,
        preliminary.principal(),
        id,
        Permission::MachineManage,
    )
    .await?;
    if !rows::lock_resource(&mut conn, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut conn).await?;
    let row = own_machine(&ctx, &mut conn, id).await?;
    require_machine(&mut conn, guard.principal(), id, Permission::MachineManage).await?;
    if body
        .expected_revision
        .is_some_and(|expected| expected != row.revision)
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The machine changed. Read its current roles and try again.",
        )
        .with("current", json!({"id":row.id,"revision":row.revision})));
    }
    require_online(&ctx, &row)?;
    if let Some(detail) = role_change_refusal(&mut conn, id, &body.roles)
        .await
        .map_err(ApiError::engine)?
    {
        return Err(ApiError::invalid("roles", detail));
    }
    let command = Outgoing::new(
        orch_types::new_id(),
        id,
        SerialKey::Machine,
        &NativeRolesSet { roles: body.roles },
    )
    .map_err(ApiError::internal)?;
    let authority = MachineRolesAuthority::capture(
        &mut conn,
        guard.principal().caller(),
        credential,
        raw_until,
        &command,
        ctx.app.clock.as_ref(),
    )
    .await
    .map_err(ApiError::internal)?;
    // Claim the guarded server snapshot before dispatch, keeping the revision off the wire.
    guard.check_deadline(&ctx)?;
    sqlx::query("UPDATE machines SET revision=revision+1 WHERE id=$1 AND revision=$2")
        .bind(id)
        .bind(row.revision)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    guard.check_deadline(&ctx)?;
    conn.commit().await.map_err(DbError::from)?;
    let dispatcher = ctx.app.link.dispatcher();
    let outcome = async {
        dispatcher
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    id: Some(command.id),
                    machine: id,
                    kind: command.kind.as_str().into(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body.clone(),
                    authority: Some(CommandAuthority::machine_roles(authority.clone())),
                },
                Box::new(|| {
                    guard.check_deadline(&ctx).map_err(|error| {
                        Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                    })
                }),
            )
            .await?;
        dispatcher.wait_for::<NativeRolesSet>(command.id).await
    }
    .await;
    let answer = outcome.map_err(|failure| match failure {
        RunError::Command(CommandFailure::Failed(refusal)) => {
            ApiError::invalid("roles", &refusal.detail)
        }
        other => command_failed(id, other),
    })?;
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    if !rows::lock_resource(&mut tx, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    require_machine(&mut tx, guard.principal(), id, Permission::MachineManage).await?;
    let (accepted, until) = authority
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the original role selection has ended",
            )
        })?;
    guard.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    let who = actor_name(&ctx)?;
    audit_machine(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::MachineRolesChanged),
        id,
        &format!("{who} changed the roles of {}", row.name),
        json!({"roles":&answer.roles,"reconnecting":answer.reconnecting}),
    )
    .await?;
    guard.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::MachineRolesResponse::Receipt(api::MachineRolesReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
        requested_roles: answer.roles.clone(),
        reconnecting: answer.reconnecting,
    });
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    Ok(
        match acknowledgement::project::<Content>(&ctx, &accepted, id).await? {
            Some(machine_view) => {
                api::MachineRolesResponse::Full(Box::new(api::MachineRolesChange {
                    machine_view,
                    requested_roles: answer.roles,
                    reconnecting: answer.reconnecting,
                }))
            }
            None => receipt,
        },
    )
}
