//! A source-bound primary selection keeps the enrolled identity and acknowledges only its native choice.

use super::*;
use crate::{
    key_operations::{native, native::Stored, targets},
    middleware::transaction_guard,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    common::ReceiptProjection,
    gateways as gateway_api,
    machine_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_engine::commands::{CommandAuthority, MachineGatewayAuthority, Outgoing};
use orch_link_contract::{
    commands::MachineGatewaySet as NativeSet,
    gateways::{GatewayChoice, GatewaySelection},
};
use orch_settings_contract::registry::keys;

struct Changes;
async fn require_manage(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    if orch_authz::resources::check_machine(
        conn,
        principal,
        id,
        Permission::MachineManage,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed()
    {
        return Ok(());
    }
    Err(ApiError::new(
        ErrorCode::AuthzDenied,
        "You cannot change this machine's gateway.",
    ))
}
impl Stored for Changes {
    type Source = Source;
    type Value = Id;
    const ROWS: &'static [Declaration<Source>] = &[sources::GATEWAY];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(Id, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("machine_id")?;
            Ok((
                id,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ResourceKind::Machine.into()),
                    targets::machine_subject(conn, id).await?,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_manage(conn, principal, *id))
    }
}
pub(super) fn register(registry: &mut Registry) {
    registry.on_with_optional_operation(
        gateway_api::MachineGatewaySet,
        native::stored_deferred::<Changes, _, _>,
        change,
    );
}

async fn change(
    ctx: Ctx,
    (): (),
    body: gateway_api::MachineGatewaySetBody,
) -> ApiResult<gateway_api::MachineGatewaySetReceipt> {
    let choice = GatewayChoice {
        selection: match body.selection {
            gateway_api::ConnectorGatewaySelection::Automatic => GatewaySelection::Automatic,
            gateway_api::ConnectorGatewaySelection::Selected => GatewaySelection::Selected,
        },
        gateway_id: body.gateway_id.clone(),
    };
    match (&choice.selection, &choice.gateway_id) {
        (GatewaySelection::Automatic, None) => {}
        (GatewaySelection::Selected, Some(id))
            if ctx
                .app
                .link
                .certificates()
                .gateway_discovery()
                .gateways
                .iter()
                .any(|gateway| gateway.id == *id) => {}
        _ => {
            return Err(ApiError::invalid(
                "gateway_id",
                "Choose Automatic or one advertised gateway.",
            ));
        }
    }
    let id = ctx.id("machine_id")?;
    let (credential, raw_until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, preliminary) = transaction_guard::begin(&ctx).await?;
    require_manage(&mut tx, preliminary.principal(), id).await?;
    if !rows::lock_resource(&mut tx, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    require_manage(&mut tx, guard.principal(), id).await?;
    let row = live_machine(&mut tx, id).await?;
    require_online(&ctx, &row)?;
    let seconds = orch_settings::get::<u64>(
        &mut tx,
        orch_settings::Place::System,
        keys::GATEWAYS_PRIMARY_CHANGE_TIMEOUT_SECS,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("The gateway change wait default is missing."))?;
    let span =
        chrono::Duration::from_std(std::time::Duration::from_secs(seconds)).map_err(|_| {
            ApiError::invalid("timeout", "The gateway change wait cannot be represented.")
        })?;
    let until = ctx.now.checked_add_signed(span).ok_or_else(|| {
        ApiError::invalid("timeout", "The gateway change wait cannot be represented.")
    })?;
    let mut command = Outgoing::new(
        orch_types::new_id(),
        id,
        SerialKey::Machine,
        &NativeSet {
            choice: choice.clone(),
        },
    )
    .map_err(ApiError::internal)?;
    command.deadline = Some(Time::new(until));
    let authority = MachineGatewayAuthority::capture(
        &mut tx,
        guard.principal().caller(),
        credential,
        raw_until,
        &command,
        ctx.app.clock.as_ref(),
    )
    .await
    .map_err(ApiError::internal)?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let dispatcher = ctx.app.link.dispatcher();
    dispatcher
        .issue_stored_before(
            crate::dispatch::StoredCommand {
                id: Some(command.id),
                machine: id,
                kind: command.kind.as_str().into(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body.clone(),
                authority: Some(CommandAuthority::machine_gateway(authority.clone())),
            },
            Box::new(|| {
                guard
                    .check_deadline(&ctx)
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            }),
        )
        .await
        .map_err(|error| command_failed(id, RunError::Dispatch(error)))?;
    let answer = dispatcher
        .wait_for::<NativeSet>(command.id)
        .await
        .map_err(|error| command_failed(id, error))?;
    if answer.choice != choice {
        return Err(ApiError::internal(
            "The native gateway acknowledgement differs from its request.",
        ));
    }
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    if !rows::lock_resource(&mut tx, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, fresh) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    require_manage(&mut tx, fresh.principal(), id).await?;
    let (accepted, until) = authority
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "The original gateway selection has ended.",
            )
        })?;
    fresh.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    let who = actor_name(&ctx)?;
    let called = choice.gateway_id.as_deref().unwrap_or("Automatic");
    audit_machine(
        &ctx,
        &mut tx,
        AuditAction::MachineGatewayChanged,
        id,
        &format!(
            "{who} changed the primary gateway of {} to {called}",
            row.name
        ),
        json!({"choice":choice,"changed":answer.changed,"command":command.id}),
    )
    .await?;
    fresh.check_deadline(&ctx)?;
    acknowledgement::deadline(&ctx, &accepted, until)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(gateway_api::MachineGatewaySetReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
        selection: body.selection,
        gateway_id: body.gateway_id,
        changed: answer.changed,
    })
}
