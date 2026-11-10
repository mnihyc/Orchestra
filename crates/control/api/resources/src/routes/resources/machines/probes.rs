//! One native facts probe, with original authority at queue, send, persistence and response.

use super::command_failed;

use super::mutations;

use super::own_machine;

use super::queries_set_facts;

use super::require_online;

use crate::key_operations::native;

use crate::key_operations::native::Stored;

use crate::key_operations::native::resource_reply::StoredResource;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::machine_sources as sources;

use orch_api_contract::machine_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::resources::MachineProbeReceipt;

use orch_api_contract::resources::MachineProbeResponse;

use orch_api_contract::resources::Resource;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::MachineProbeAuthority;

use orch_engine::commands::Outgoing;

use orch_link_contract::commands::MachineProbe;

use orch_link_contract::commands::SerialKey;

use orch_link_contract::session::MachineFacts;

use orch_types::Id;

use orch_types::new_id;
pub(super) struct Probes;

impl Stored for Probes {
    type Source = Source;
    type Value = mutations::Subject;
    const ROWS: &'static [Declaration<Source>] = &[sources::PROBE];
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

impl StoredResource for Probes {
    const READ_SOURCE: Source = Source::Governor;
    const ROLES: &'static single::Roles = <mutations::Machines as StoredResource>::ROLES;
    fn object(value: &mutations::Subject) -> ObjectRef {
        <mutations::Machines as StoredResource>::object(value)
    }
    fn require_read<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a mutations::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <mutations::Machines as StoredResource>::require_read(ctx, conn, principal, value)
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        opened: &'a mutations::Subject,
        fresh: &'a mutations::Subject,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(fresh.covers(opened)) })
    }
}

fn receipt(id: Id) -> MachineProbeResponse {
    MachineProbeResponse::Receipt(MachineProbeReceipt {
        projection: ReceiptProjection::Receipt,
        machine_id: id,
    })
}

fn ended() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the original machine probe has ended",
    )
}

fn deadline(
    ctx: &Ctx,
    caller: &orch_authz::Caller,
    until: Option<orch_types::Time>,
) -> ApiResult<()> {
    if !caller.key_deadline_allows(ctx.app.clock.as_ref())
        || until.is_some_and(|end| end.get() <= ctx.app.clock.now())
    {
        return Err(ended());
    }
    Ok(())
}

pub(super) async fn probe(ctx: Ctx, (): (), (): ()) -> ApiResult<MachineProbeResponse> {
    let id = ctx.id("id")?;
    let (credential, raw_until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let subject = Probes::load(&ctx, &mut tx, &sources::PROBE).await?.0;
    Probes::require(&ctx, &mut tx, guard.principal(), &subject).await?;
    let row = own_machine(&ctx, &mut tx, id).await?;
    require_online(&ctx, &row)?;
    let command = Outgoing::new(new_id(), id, SerialKey::Machine, &MachineProbe {})
        .map_err(ApiError::internal)?;
    let authority = MachineProbeAuthority::capture(
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
    let run = async {
        dispatcher
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    id: Some(command.id),
                    machine: id,
                    kind: command.kind.as_str().into(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    body: command.body.clone(),
                    authority: Some(CommandAuthority::machine_probe(authority.clone())),
                },
                Box::new(|| {
                    guard.check_deadline(&ctx).map_err(|error| {
                        Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                    })
                }),
            )
            .await?;
        dispatcher.wait_for::<MachineProbe>(command.id).await
    };
    #[cfg(feature = "test-observers")]
    let outcome = orch_link::test_observers::probe_request(id, &ctx.request_id, run).await;
    #[cfg(not(feature = "test-observers"))]
    let outcome = run.await;
    let facts: MachineFacts = outcome.map_err(|error| command_failed(id, error))?;
    let written = serde_json::to_value(&facts).map_err(ApiError::internal)?;
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    if !orch_db::queries::resources::lock_resource(&mut tx, "machine", id).await? {
        return Err(ApiError::not_found());
    }
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let (caller, until) = authority
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ended)?;
    guard.check_deadline(&ctx)?;
    deadline(&ctx, &caller, until)?;
    queries_set_facts(&mut tx, id, &written, &facts, ctx.app.clock.now()).await?;
    guard.check_deadline(&ctx)?;
    deadline(&ctx, &caller, until)?;
    tx.commit().await.map_err(DbError::from)?;
    crate::key_operations::publication::receipt(&ctx, &receipt(id))?;
    match native::resource_reply::stored::<Probes>(&ctx, &caller).await? {
        Some(Resource::Machine(machine)) => Ok(MachineProbeResponse::Full(Box::new(machine))),
        None => Ok(receipt(id)),
        Some(_) => Err(ApiError::internal(
            "the probed Machine response has another resource kind",
        )),
    }
}
