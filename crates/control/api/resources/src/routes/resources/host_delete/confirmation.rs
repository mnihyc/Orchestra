//! Confirmation binds the actual freshly prepared cascade and preserves the native writer.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use futures::future::BoxFuture;

use orch_api_contract::host_delete_sources::CREATE;

use orch_api_contract::host_delete_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use std::sync::Arc;
pub(super) struct Confirm;

pub(super) struct Input {
    stored: Stored,
    body: api::HostDeleteCreateBody,
}

pub(super) struct Ready {
    prepared: Arc<PreparedCreation>,
}

pub(super) struct Started {
    native: Arc<PreparedCreation>,
    job: Id,
}

async fn reviewed(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: api::HostDeleteCreateBody,
) -> ApiResult<Input> {
    let row = jobs::forks::preflight(conn, body.preflight_id)
        .await?
        .filter(|row| {
            row.kind == "host_delete"
                && row.created_by == caller.user()
                && Time::new(row.expires_at) > ctx.app.clock.time()
        })
        .ok_or_else(ApiError::not_found)?;
    let stored: Stored = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
    if stored.host != ctx.id("id")? {
        return Err(ApiError::not_found());
    }
    if body.confirm != stored.plan.name {
        return Err(ApiError::invalid("confirm", "type the host's exact name"));
    }
    Ok(Input { stored, body })
}

impl effects::Initial for Confirm {
    type Input = Input;
    type Fetched = PreparedCreation;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &CREATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let input = reviewed(ctx, conn, caller, body).await?;
            let facts = source::required(conn, &input.stored).await?;
            Ok((input, facts))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { source::require(conn, principal, &input.stored).await })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<PreparedCreation>> {
        Box::pin(async move {
            let (principal, planning) = {
                let mut tx = ctx.app.db.begin_snapshot().await?;
                let facts = source::required(&mut tx, &input.stored).await?;
                let known = native::bind_known(ctx, &CREATE, &facts)?;
                let principal = native::preparation_principal(ctx, &mut tx, caller, &known).await?;
                let captured = orch_authz::CapturedOperation::capture(principal.caller())
                    .map_err(ApiError::internal)?;
                let http = native::http_origin::capture_operation(ctx, &mut tx, &captured).await?;
                let planning = orch_engine::jobs::lifecycle_commands::planning::Planning::host(
                    &mut tx,
                    http,
                    input.body.preflight_id,
                    input.stored.host,
                    ctx.app.clock.as_ref(),
                )
                .await
                .map_err(ApiError::engine)?;
                tx.rollback().await.map_err(DbError::from)?;
                (principal, planning)
            };
            let origin = operation::authority::Origin::from_http(planning.retained())
                .map_err(ApiError::engine)?;
            let mut prepared = planning
                .scope(prepare_creation(ctx, &principal, input.body, origin))
                .await?;
            let http = planning.retained();
            let mut tail = ctx.app.db.begin().await?;
            let _ = native::http_origin::current(ctx, &mut tail, &http).await?;
            tail.rollback().await.map_err(DbError::from)?;
            prepared.origin =
                operation::authority::Origin::from_http(http).map_err(ApiError::engine)?;
            Ok(prepared)
        })
    }
}

impl effects::Effect for Confirm {
    type Source = Source;
    type Query = ();
    type Body = api::HostDeleteCreateBody;
    type Value = Ready;
    type Output = Started;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let prepared = effects::initial_value::<Self>(ctx)?;
            if prepared.stored.host != ctx.id("id")?
                || prepared.body != body
                || prepared.preflight.created_by != caller.user()
            {
                return Err(ApiError::not_found());
            }
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                orch_db::queries::resources::lock_resource(conn, "host", prepared.stored.host)
                    .await?;
                let footprint = source::footprint(conn, &prepared.stored).await?;
                for (kind, id) in footprint.managed {
                    match kind {
                        ObjectKind::Environment => {
                            envs::lock_environment(conn, id).await?;
                        }
                        ObjectKind::Provider => {
                            orch_db::queries::resources::lock_resource(conn, "provider", id)
                                .await?;
                        }
                        _ => {
                            return Err(ApiError::internal(
                                "unsupported native cascade management source",
                            ));
                        }
                    }
                }
                jobs::changes::lock_plan(conn, body.preflight_id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
            }
            let row = jobs::forks::preflight(conn, body.preflight_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.plan != prepared.preflight.plan
                || row.created_by != caller.user()
                || row.kind != "host_delete"
                || Time::new(row.expires_at) <= ctx.app.clock.time()
            {
                return Err(refused(UpdateError::Expired));
            }
            let inventory = loss::inventory(conn, "host", prepared.stored.host)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if inventory.fingerprint()? != prepared.stored.fingerprint {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "The host's dependencies changed; review its deletion again",
                ));
            }
            let facts = source::required(conn, &prepared.stored).await?;
            Ok((Ready { prepared }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if Time::new(value.prepared.preflight.expires_at) <= ctx.app.clock.time() {
                return Err(refused(UpdateError::Expired));
            }
            source::require(conn, principal, &value.prepared.stored).await
        })
    }
}

impl effects::ApplyOwned for Confirm {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Started>>> {
        Box::pin(async move {
            let job = apply_creation(ctx, tx, principal, &value.prepared, before).await?;
            Ok(effects::Outcome::Applied(Started {
                native: value.prepared.clone(),
                job: job.id,
            }))
        })
    }
}

enum Readback {
    First(Started),
    Replay { job: Id, host: Id, account: Id },
}

impl Readback {
    fn host(&self) -> Id {
        match self {
            Self::First(s) => s.native.stored.host,
            Self::Replay { host, .. } => *host,
        }
    }
    fn job(&self) -> Id {
        match self {
            Self::First(s) => s.job,
            Self::Replay { job, .. } => *job,
        }
    }
    fn account(&self) -> Id {
        match self {
            Self::First(s) => s.native.preflight.created_by,
            Self::Replay { account, .. } => *account,
        }
    }
}

struct Content;

impl reply::Projection for Content {
    type Source = Source;
    type Input = Readback;
    type Subject = ();
    type Value = jobs::JobRow;
    type Reply = Job;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Readback) -> &'static Declaration<Source> {
        &CREATE
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let facts = match input {
                Readback::First(input) => {
                    let mut facts = source::required(conn, &input.native.stored).await?;
                    facts.push(source::content(conn, &input.native.stored).await?);
                    facts
                }
                Readback::Replay { job, host, account } => {
                    let row = jobs::job(conn, *job)
                        .await?
                        .filter(|r| r.kind == operation::KIND && r.created_by == Some(*account))
                        .ok_or_else(ApiError::not_found)?;
                    let native: operation::Recipe =
                        serde_json::from_value(row.plan["cascade"].clone())
                            .map_err(ApiError::internal)?;
                    if native.host != *host || native.by != *account {
                        return Err(ApiError::not_found());
                    }
                    vec![source::objects(conn, Source::Content, [(ObjectKind::Host, *host)]).await?]
                }
            };
            Ok(((), facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            operation_access::review(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: input.host(),
                },
                Permission::HostView,
            )
            .await
            .map_err(refused)?;
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Readback,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(jobs::JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.job())
                .await?
                .filter(|row| {
                    row.kind == operation::KIND && row.created_by == Some(input.account())
                })
                .ok_or_else(ApiError::not_found)?;
            Ok((row, Vec::new()))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move { crate::routes::environments::jobs::view(conn, &value.value).await })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        before: &'a reply::Prepared<Self>,
        after: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(before.value.id == after.value.id
                && before.value.plan == after.value.plan
                && before.value.updated_at == after.value.updated_at)
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::HostDeleteCreateBody,
) -> ApiResult<api::HostDeleteCreateResponse> {
    let tx = ctx.app.db.begin().await?;
    let done = effects::apply_owned::<Confirm>(&ctx, tx).await?;
    let receipt = api::HostDeleteCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        host_id: done.output.native.stored.host,
        job_id: done.output.job,
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Content>(&done.ctx, &done.accepted, Readback::First(done.output)).await? {
        Some(full) => Ok(api::HostDeleteCreateResponse::Full(Box::new(full))),
        None => Ok(api::HostDeleteCreateResponse::Receipt(receipt)),
    }
}

const REPLAY_KIND: &str = "host_delete_job";

pub(super) fn replay_binding() -> crate::middleware::idempotency::Creation<
    (),
    api::HostDeleteCreateBody,
    api::HostDeleteCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: REPLAY_KIND,
        replay,
        identity: |value| {
            let id = match value {
                api::HostDeleteCreateResponse::Full(v) => v.id,
                api::HostDeleteCreateResponse::Receipt(v) => v.job_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: REPLAY_KIND.into(),
                id,
                data: serde_json::Value::Null,
            })
        },
    }
}

fn replay(
    ctx: Ctx,
    (): (),
    _: api::HostDeleteCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<api::HostDeleteCreateResponse>> {
    Box::pin(async move {
        if created.kind != REPLAY_KIND {
            return Err(ApiError::internal(
                "the accepted host deletion kind changed",
            ));
        }
        let host = ctx.id("id")?;
        let account = ctx.user()?.id();
        let receipt = api::HostDeleteCreateResponse::Receipt(api::HostDeleteCreateReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            host_id: host,
            job_id: created.id,
        });
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<Content>(
            &ctx,
            account,
            Readback::Replay {
                job: created.id,
                host,
                account,
            },
        )
        .await
        {
            Ok(Some(job)) => Ok(api::HostDeleteCreateResponse::Full(Box::new(job))),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}

pub(super) async fn observed(conn: &mut Conn, account: Id, job: Id) -> ApiResult<()> {
    orch_db::idempotency::creation::observe(conn, Some(account), REPLAY_KIND, job).await?;
    Ok(())
}
