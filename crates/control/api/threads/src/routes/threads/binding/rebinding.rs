//! Native binding selection, once-only descriptor startup, original first stamp and current Thread publication.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::routes::threads::content_reply;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::binding::ThreadBindingReceipt;

use orch_api_contract::binding::ThreadBindingResponse;

use orch_api_contract::binding_sources as rows;

use orch_api_contract::binding_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_engine::binding::PreparedRebind;

use orch_engine::binding::operation as source;
const ROW: &Declaration<Source> = &rows::REBIND.sources;

pub(super) struct Bind;

pub(super) struct Input {
    source: source::Input,
    missing: bool,
}

pub(super) struct Fetched {
    origin: source::Origin,
}

pub(super) struct Value {
    native: PreparedRebind,
    origin: source::Origin,
}

pub(super) struct Accepted {
    receipt: ThreadBindingReceipt,
    row: thread_rows::ThreadRow,
    origin: source::Origin,
}

async fn facts(conn: &mut Conn, id: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::node(conn, id).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
        return Err(ApiError::not_found());
    }
    Ok(vec![native::Facts::one(
        Source::Thread,
        target.object.kind,
        target,
    )])
}

fn engine_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::BeforeEffect(error) => match error.downcast::<ApiError>() {
            Ok(error) => *error,
            Err(_) => ApiError::new(
                ErrorCode::AuthzDenied,
                "the original binding source is no longer available",
            ),
        },
        error => ApiError::engine(error),
    }
}

async fn problem(ctx: &Ctx, conn: &mut Conn, id: Id, error: RebindError) -> ApiError {
    let error = match error {
        RebindError::Internal(error) => engine_error(error),
        error => rebind_failed(conn, id, error).await,
    };
    if ctx.key_receipt_required() {
        error.without_content()
    } else {
        error
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ThreadBindingSetBody,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let plan = effects::prepare::<Bind>(ctx, conn, caller, query, body).await?;
        let fetched = effects::initial_value::<Bind>(ctx)?;
        plan.with_origin(ctx, conn, caller, fetched.origin.captured())
            .await
    })
}

fn request(body: &ThreadBindingSetBody) -> Rebind {
    Rebind {
        agent: body.agent_id,
        provider: body.provider_id,
        arrangement: same(&body.arrangement),
        model: body.model.clone(),
        effort: body.effort.clone(),
    }
}

impl effects::Initial for Bind {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(_input: &Input) -> &'static Declaration<Source> {
        ROW
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadBindingSetBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let facts = facts(conn, id).await?;
            let known = native::bind_known(ctx, ROW, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let missing = match ctx
                .app
                .engine
                .selecting_rebind(conn, &principal, id, &request(&body))
                .await
            {
                Ok(_) => false,
                Err(RebindError::AgentNotReported) => true,
                Err(why) => return Err(problem(ctx, conn, id, why).await),
            };
            let source = source::Input::select(conn, id, body)
                .await
                .map_err(engine_error)?;
            Ok((Input { source, missing }, facts))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let (credential, until) =
                crate::routes::threads::adopt::operation::http_origin(ctx).await?;
            let origin = source::Origin::capture(caller, input.source, credential, until)
                .map_err(engine_error)?;
            if !input.missing {
                return Ok(Fetched { origin });
            }
            let live = ctx.app.engine.binding_preparation(origin);
            let startup = live.startup();
            let source = live.retained().map_err(engine_error)?;
            let learned = ctx
                .app
                .engine
                .learn_descriptor_with_preparation(
                    source.input().body().agent_id,
                    source.input().body().provider_id,
                    Some(source.input().environment()),
                    Some(&startup),
                )
                .await;
            let learned = match learned {
                Ok(learned) => learned,
                Err(why) => {
                    // Keep the native HTTP authentication answer after its real transport ended during startup.
                    if matches!(&why, orch_engine::EngineError::BeforeEffect(_)) {
                        let mut tx = ctx.app.db.begin().await?;
                        let identity = transaction_guard::current_http_identity(
                            ctx,
                            &mut tx,
                            transaction_guard::HttpIdentityMode::Read,
                        )
                        .await;
                        tx.rollback().await.map_err(orch_db::DbError::from)?;
                        let _ = identity?;
                    }
                    return Err(engine_error(why));
                }
            };
            if !matches!(learned, Learning::Known) {
                let mut conn = ctx.app.db.acquire().await?;
                return Err(problem(
                    ctx,
                    &mut conn,
                    source.input().thread(),
                    RebindError::AgentNotReported,
                )
                .await);
            }
            Ok(Fetched {
                origin: live.retained().map_err(engine_error)?,
            })
        })
    }
}

impl effects::Effect for Bind {
    type Source = Source;
    type Query = ();
    type Body = ThreadBindingSetBody;
    type Value = Value;
    type Output = Accepted;
    fn declaration(_value: &Value) -> &'static Declaration<Source> {
        ROW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadBindingSetBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Bind>(ctx)?;
            if fetched.origin.input().thread() != ctx.id("id")?
                || fetched.origin.input().body() != &body
            {
                return Err(ApiError::internal("the native binding has another request"));
            }
            thread_rows::lock(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let facts = fetched
                .origin
                .input()
                .facts(conn)
                .await
                .map_err(engine_error)?;
            let plan = native::bind_required(ctx, ROW, &facts)?
                .with_origin(ctx, conn, caller, fetched.origin.captured())
                .await?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            fetched
                .origin
                .deadline(ctx.app.clock.as_ref(), principal.caller())
                .map_err(engine_error)?;
            let native = match ctx
                .app
                .engine
                .preparing_rebind(conn, &principal, ctx.id("id")?, &request(&body))
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(problem(ctx, conn, ctx.id("id")?, why).await),
            };
            Ok((
                Value {
                    native,
                    origin: fetched.origin.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            value
                .origin
                .deadline(ctx.app.clock.as_ref(), principal.caller())
                .map_err(engine_error)
        })
    }
}

impl effects::Apply for Bind {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let result = match ctx
                .app
                .engine
                .write_rebind_before(conn, principal, &value.native, || {
                    value
                        .origin
                        .deadline(ctx.app.clock.as_ref(), principal.caller())?;
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(value) => value,
                Err(why) => {
                    return Err(problem(ctx, conn, value.native.thread().node_id, why).await);
                }
            };
            let row = thread_rows::thread(conn, result.thread)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok(effects::Outcome::Applied(Accepted {
                receipt: ThreadBindingReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    thread_id: result.thread,
                    seq: result.seq,
                },
                row,
                origin: value.origin.clone(),
            }))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _body: ThreadBindingSetBody,
) -> ApiResult<ThreadBindingResponse> {
    let result = change(&ctx).await;
    result.map_err(|error| {
        if ctx.key_receipt_required() {
            error.without_content()
        } else {
            error
        }
    })
}

async fn change(ctx: &Ctx) -> ApiResult<ThreadBindingResponse> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let done = effects::apply::<Bind>(ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    done.output
        .origin
        .deadline(done.ctx.app.clock.as_ref(), &done.accepted)
        .map_err(engine_error)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    done.ctx.app.engine.wake(done.output.receipt.thread_id);
    publication::receipt(&done.ctx, &done.output.receipt)?;
    let receipt = done.output.receipt.clone();
    match reply::render::<Readback>(&done.ctx, &done.accepted, done.output).await? {
        Some(thread) => Ok(ThreadBindingResponse::Full(Box::new(thread))),
        None => Ok(ThreadBindingResponse::Receipt(receipt)),
    }
}

fn same_binding(old: &thread_rows::ThreadRow, new: &thread_rows::ThreadRow) -> bool {
    old.node_id == new.node_id
        && old.agent_id == new.agent_id
        && old.provider_id == new.provider_id
        && old.environment_id == new.environment_id
        && old.active_continuation_id == new.active_continuation_id
        && old.current_segment_id == new.current_segment_id
        && old.execution_revision == new.execution_revision
        && old.workspace_dir == new.workspace_dir
}

struct Readback;

impl reply::Projection for Readback {
    type Source = Source;
    type Input = Accepted;
    type Subject = thread_rows::ThreadRow;
    type Value = crate::routes::nodes::content_sources::Prepared;
    type Reply = Thread;
    const READ_SOURCE: Source = Source::Thread;
    fn declaration(_input: &Accepted) -> &'static Declaration<Source> {
        ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = thread_rows::thread(conn, input.receipt.thread_id)
                .await?
                .filter(|row| row.deleted_at.is_none() && same_binding(&input.row, row))
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, row.node_id).await?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Accepted,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if native::modern(principal.caller()) {
                authorization::require(
                    conn,
                    principal,
                    subject.node_id,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Accepted,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (value, groups) = content_reply::prepare(conn, principal, subject.node_id).await?;
            Ok((
                value,
                vec![native::Facts::collection(Source::ThreadContent, groups)],
            ))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a Accepted,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Thread>> {
        Box::pin(async move {
            content_reply::render(
                ctx,
                conn,
                principal,
                prepared.subject.node_id,
                &prepared.value,
            )
            .await
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(same_binding(&old.subject, &fresh.subject) && old.value.preserves(&fresh.value))
        })
    }
}
