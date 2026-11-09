//! Native Host control, its exact deferred actor and optional same-operation status.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::authentication::Via;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::routes::resources::machines;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::host_actions;

use orch_api_contract::host_actions::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;
pub(super) struct Update;

pub(super) struct Value {
    host: Id,
    physical: machines::PreparedUpdate,
}

pub(super) struct Output {
    host: Id,
    machine: Id,
    fresh: bool,
}

async fn fact(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::Host,
        KeyTargetKind::Object(ResourceKind::Host.into()),
        targets::page_object(
            conn,
            ObjectRef {
                kind: ResourceKind::Host.into(),
                id,
            },
        )
        .await?,
    ))
}

impl effects::Effect for Update {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Value;
    type Output = Output;
    fn declaration(_value: &Value) -> &'static Declaration<Source> {
        &host_actions::UPDATE_NOW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let host = ctx.id("id")?;
            let known =
                native::bind_known(ctx, &host_actions::UPDATE_NOW, &[fact(conn, host).await?])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require(
                conn,
                &principal,
                ResourceKind::Host,
                host,
                Permission::HostManage,
            )
            .await?;
            let row = queries::hosts(conn, &[host], principal.user())
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                orch_db::queries::component_updates::lock(conn, row.machine_id).await?;
                let _ = sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
                    .bind(host)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                let current = queries::hosts(conn, &[host], principal.user())
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)?;
                if current.machine_id != row.machine_id {
                    return Err(DbError::Retry.into());
                }
            }
            let physical =
                machines::prepare_update_on(ctx, conn, &principal, row.machine_id).await?;
            Ok((Value { host, physical }, vec![fact(conn, host).await?]))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            principal,
            ResourceKind::Host,
            value.host,
            Permission::HostManage,
        ))
    }
}

impl effects::ApplyOwned for Update {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let outcome =
                machines::apply_update_on(ctx, &mut tx, principal, &value.physical, || {
                    before.check()
                })
                .await?;
            if outcome.fresh {
                let identity =
                    transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                        .await?;
                let session = match &identity.user.via {
                    Via::Session { session_id } => Some(*session_id),
                    Via::Token { .. } | Via::OwnerRead { .. } => None,
                };
                let origin = orch_engine::component_update_origin::Origin::capture(
                    &mut tx,
                    principal.caller(),
                    value.host,
                    value.physical.machine(),
                    session,
                    identity.expires_at,
                )
                .await
                .map_err(ApiError::internal)?;
                origin.store(&mut tx).await.map_err(ApiError::internal)?;
                identity.check_deadline(&ctx.app)?;
            }
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(Output {
                host: value.host,
                machine: value.physical.machine(),
                fresh: outcome.fresh,
            }))
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), (): ()) -> ApiResult<api::HostUpdateNowResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Update>(&ctx, tx).await?;
    let restricted = api::HostUpdateNowResponse::Receipt(api::HostUpdateNowReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        host_id: completed.output.host,
        machine_id: completed.output.machine,
        requested: completed.output.fresh,
    });
    publication::receipt(&completed.ctx, &restricted)?;
    let projected = native::observed(
        &completed.ctx,
        "host.update.reply",
        reply::render::<Status>(&completed.ctx, &completed.accepted, completed.output).await,
    )?;
    match projected {
        Some(view) => Ok(api::HostUpdateNowResponse::full(view)),
        None => Ok(restricted),
    }
}

struct Status;

struct Subject {
    host: Id,
    machine: Id,
}

struct Shown {
    view: api::ComponentUpdate,
    request: Option<Id>,
}

impl reply::Projection for Status {
    type Source = Source;
    type Input = Output;
    type Subject = Subject;
    type Value = Shown;
    type Reply = api::ComponentUpdate;
    const READ_SOURCE: Source = Source::Host;
    fn declaration(_input: &Output) -> &'static Declaration<Source> {
        &host_actions::UPDATE_NOW
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Output,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let machine: Option<Id> = native::observed(
                ctx,
                "host.status.physical",
                sqlx::query_scalar(
                    "SELECT machine_id FROM hosts WHERE id=$1 AND deleted_at IS NULL",
                )
                .bind(input.host)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)
                .map_err(ApiError::from),
            )?;
            if machine != Some(input.machine) {
                return native::observed(
                    ctx,
                    "host.status.association",
                    Err(ApiError::not_found()),
                );
            }
            let subject = (
                Subject {
                    host: input.host,
                    machine: input.machine,
                },
                vec![fact(conn, input.host).await?],
            );
            native::observed(ctx, "host.status.subject", Ok(subject))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Output,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            native::observed(
                ctx,
                "host.status.management",
                require(
                    conn,
                    principal,
                    ResourceKind::Host,
                    subject.host,
                    Permission::HostManage,
                )
                .await,
            )
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Output,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(Shown, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let view = native::observed(
                ctx,
                "host.status.view",
                crate::component_updates::views(&ctx.app.link, conn, &[subject.machine])
                    .await
                    .map_err(ApiError::from),
            )?
            .remove(&subject.machine)
            .ok_or_else(ApiError::not_found)?;
            let request = native::observed(
                ctx,
                "host.status.request",
                sqlx::query_scalar("SELECT request_id FROM component_updates WHERE machine_id=$1")
                    .bind(subject.machine)
                    .fetch_optional(conn)
                    .await
                    .map_err(DbError::from)
                    .map_err(ApiError::from),
            )?;
            Ok((Shown { view, request }, Vec::new()))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Output,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::ComponentUpdate>> {
        Box::pin(async move {
            native::observed(ctx, "host.status.render", Ok(value.value.view.clone()))
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Output,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.host == fresh.subject.host
                && old.subject.machine == fresh.subject.machine
                && old.value.request == fresh.value.request)
        })
    }
}
