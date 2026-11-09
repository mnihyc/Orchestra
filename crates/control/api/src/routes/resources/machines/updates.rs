//! Explicit updates inside the native physical drain fence, with exact deferred issuer provenance.

use super::*;
use crate::key_operations::{native, targets};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    common::ReceiptProjection,
    machine_sources::{self as sources, Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_engine::component_update_authority::{Intent, Origin};

pub(super) struct Requests;
impl native::Stored for Requests {
    type Source = Source;
    type Value = MachineRow;
    const ROWS: &'static [Declaration<Source>] = &[sources::UPDATE_NOW];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(MachineRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live_machine(conn, ctx.id("id")?).await?;
            let target = targets::machine_subject(conn, row.id).await?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_machine(
            conn,
            principal,
            value.id,
            Permission::MachineManage,
        ))
    }
}
struct Projection;
impl native::reply::Projection for Projection {
    type Source = Source;
    type Input = Id;
    type Subject = MachineRow;
    type Value = ();
    type Reply = api::ComponentUpdate;
    const READ_SOURCE: Source = Source::Governor;
    fn declaration(_input: &Id) -> &'static Declaration<Source> {
        &sources::UPDATE_NOW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        id: &'a Id,
    ) -> BoxFuture<'a, ApiResult<(MachineRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live_machine(conn, *id).await?;
            let target = targets::machine_subject(conn, row.id).await?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    target,
                )],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: &'a Id,
        _subject: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_machine(
            conn,
            principal,
            *id,
            Permission::MachineManage,
        ))
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        _subject: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        id: &'a Id,
        _prepared: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::ComponentUpdate>> {
        Box::pin(async move {
            crate::component_updates::views(&ctx.app.link, conn, &[*id])
                .await?
                .remove(id)
                .ok_or_else(ApiError::not_found)
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _id: &'a Id,
        old: &'a native::reply::Prepared<Self>,
        fresh: &'a native::reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.owner_id == fresh.subject.owner_id
                && old.subject.host_id == fresh.subject.host_id
                && old.subject.runtime_id == fresh.subject.runtime_id)
        })
    }
}
struct Refusal;
impl acknowledgement::Body for Refusal {
    type Reply = ();
    const DECLARATION: &'static Declaration<Source> = &sources::UPDATE_NOW;
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_machine(
            conn,
            principal,
            row.id,
            Permission::MachineManage,
        ))
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _row: &'a MachineRow,
        _machine: Option<Machine>,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}
struct Completed {
    ctx: Ctx,
    accepted: Caller,
}
fn receipt(machine: Id) -> api::MachineUpdateNowResponse {
    api::MachineUpdateNowResponse::Receipt(api::MachineUpdateNowReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: machine,
    })
}
pub(super) async fn update(ctx: Ctx, (): (), (): ()) -> ApiResult<api::MachineUpdateNowResponse> {
    let projection = ctx.clone();
    match machine(ctx).await {
        Ok(value) => Ok(value),
        Err(error) => Err(acknowledgement::refusal::<Refusal>(&projection, error).await?),
    }
}
async fn machine(ctx: Ctx) -> ApiResult<api::MachineUpdateNowResponse> {
    let machine = ctx.id("id")?;
    let done = request(ctx, machine).await?;
    crate::key_operations::publication::receipt(&done.ctx, &receipt(machine))?;
    Ok(
        match native::reply::render::<Projection>(&done.ctx, &done.accepted, machine).await? {
            Some(value) => api::MachineUpdateNowResponse::Full(Box::new(value)),
            None => receipt(machine),
        },
    )
}
async fn request(ctx: Ctx, machine: Id) -> ApiResult<Completed> {
    let (credential, until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    prepare_update_on(&ctx, &mut tx, initial.principal(), machine).await?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let prepared = prepare_update_on(&ctx, &mut tx, guard.principal(), machine).await?;
    let origin = if prepared.view.behind {
        let intent = Intent {
            id: orch_types::new_id(),
            machine,
            recommended: prepared.view.recommended_version.clone(),
        };
        Some(
            Origin::machine(
                &mut tx,
                guard.principal().caller(),
                credential,
                until,
                intent,
                ctx.app.clock.as_ref(),
            )
            .await
            .map_err(ApiError::engine)?,
        )
    } else {
        None
    };
    let packet = origin
        .as_ref()
        .map(|origin| serde_json::to_value(origin).map(|value| (origin.request_id(), value)))
        .transpose()
        .map_err(ApiError::internal)?;
    apply_update_with_origin(
        &ctx,
        &mut tx,
        guard.principal(),
        &prepared,
        packet.as_ref().map(|(id, value)| (*id, value)),
        || guard.check_deadline(&ctx),
    )
    .await?;
    guard.check_deadline(&ctx)?;
    let accepted = guard.principal().caller().clone();
    tx.commit().await.map_err(DbError::from)?;
    Ok(Completed { ctx, accepted })
}
