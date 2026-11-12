//! The original adopted history through the shared initial, effect and response factories.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::middleware::transaction_guard;

use crate::registry::OperationFuture;

use crate::routes::nodes::content_sources;

use crate::routes::threads::content_reply;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::adoption_sources as rows;

use orch_api_contract::adoption_sources::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::threads::ThreadAdoptRetryHistoryReceipt;

use orch_api_contract::threads::ThreadAdoptRetryHistoryResponse;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_engine::adoption::operation as source;

use orch_engine::adoption::retry;

use orch_types::Time;
pub(super) struct Retry;

pub(super) struct Accepted {
    written: retry::Written,
    until: Option<Time>,
}

async fn problem(ctx: &Ctx, conn: &mut Conn, error: AdoptError) -> ApiError {
    if matches!(&error, AdoptError::Internal(orch_engine::EngineError::BeforeEffect(cause)) if cause.is::<source::PreparationEnded>())
    {
        return match transaction_guard::current_http_identity(
            ctx,
            conn,
            transaction_guard::HttpIdentityMode::Read,
        )
        .await
        {
            Err(error) => error,
            Ok(_) => ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the original history source no longer permits this retry",
            ),
        };
    }
    let error = failed(conn, error).await;
    if ctx.key_receipt_required() {
        error.without_content()
    } else {
        error
    }
}

async fn checked<T>(ctx: &Ctx, conn: &mut Conn, value: Result<T, AdoptError>) -> ApiResult<T> {
    match value {
        Ok(value) => Ok(value),
        Err(error) => Err(problem(ctx, conn, error).await),
    }
}

/// The shared effect resolver also retains the record observation's earlier caller minimum.
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let plan = effects::prepare::<Retry>(ctx, conn, caller, query, body).await?;
        let fetched = effects::initial_value::<Retry>(ctx)?;
        match fetched.origin() {
            Some(origin) => plan.with_origin(ctx, conn, caller, origin).await,
            None => Ok(plan),
        }
    })
}

impl effects::Effect for Retry {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = retry::Prepared;
    type Output = Accepted;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &rows::RETRY
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Retry>(ctx)?;
            if fetched.source().thread() != ctx.id("id")? {
                return Err(ApiError::internal(
                    "the retry observation has another thread",
                ));
            }
            let facts = source::retry_history_facts(conn, fetched.source())
                .await
                .map_err(ApiError::engine)?;
            let known = native::bind_required(ctx, &rows::RETRY, &facts)?;
            let known = match fetched.origin() {
                Some(origin) => known.with_origin(ctx, conn, caller, origin).await?,
                None => known,
            };
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let value = ctx
                .app
                .engine
                .prepare_retry_adopted_history(conn, &principal, fetched)
                .await;
            let value = checked(ctx, conn, value).await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let result = retry::require(conn, principal, value.source()).await;
            checked(ctx, conn, result).await
        })
    }
}

impl effects::Initial for Retry {
    type Input = retry::Source;
    type Fetched = retry::Fetched;
    fn declaration(_: &Self::Input) -> &'static Declaration<Source> {
        &rows::RETRY_PREPARATION
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Self::Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            let target = crate::key_operations::targets::node(conn, thread).await?;
            if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
                return Err(ApiError::not_found());
            }
            let primary = native::Facts::one(Source::RetainedThread, target.object.kind, target);
            let known = native::bind_known(ctx, &rows::RETRY_PREPARATION, &[primary])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let required = retry::require_thread(conn, &principal, thread).await;
            checked(ctx, conn, required).await?;
            let selected = retry::select(conn, thread).await;
            let selected = checked(ctx, conn, selected).await?;
            let facts = source::retry_history_facts(conn, &selected)
                .await
                .map_err(ApiError::engine)?;
            Ok((selected, facts))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let result = retry::require(conn, principal, input).await;
            checked(ctx, conn, result).await
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Self::Input,
    ) -> BoxFuture<'a, ApiResult<Self::Fetched>> {
        Box::pin(async move {
            let (credential, until) = super::operation::http_origin(ctx).await?;
            let value = ctx
                .app
                .engine
                .fetch_retry_adopted_history(
                    caller,
                    input,
                    credential,
                    Some(super::operation::history_transport(ctx.clone())),
                    until,
                )
                .await;
            let mut conn = ctx.app.db.acquire().await?;
            checked(ctx, &mut conn, value).await
        })
    }
}

fn deadline(ctx: &Ctx, until: Option<Time>) -> ApiResult<()> {
    if until.is_some_and(|until| ctx.app.clock.now() >= until.get()) {
        Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthInvalidToken,
            "the original history observation credential has ended",
        ))
    } else {
        Ok(())
    }
}

impl effects::ApplyOwned for Retry {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let unchanged = value.unchanged();
            let until = value.until();
            let commit = before.deadline();
            let result = ctx
                .app
                .engine
                .apply_retry_adopted_history(
                    tx,
                    principal,
                    value.clone(),
                    move || {
                        deadline(ctx, until)
                            .and_then(|()| before.check())
                            .map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })
                    },
                    move || {
                        deadline(ctx, until)
                            .and_then(|()| commit.check())
                            .map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })
                    },
                )
                .await;
            let mut conn = ctx.app.db.acquire().await?;
            let written = checked(ctx, &mut conn, result).await?;
            let value = Accepted { written, until };
            Ok(if unchanged {
                effects::Outcome::Unchanged(value)
            } else {
                effects::Outcome::Applied(value)
            })
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadAdoptRetryHistoryResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Retry>(&ctx, tx).await?;
    let receipt = ThreadAdoptRetryHistoryReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        thread_id: done.output.written.thread,
        changed: done.output.written.changed,
    };
    publication::receipt(&done.ctx, &receipt)?;
    if deadline(&done.ctx, done.output.until).is_err() {
        return Ok(ThreadAdoptRetryHistoryResponse::Receipt(receipt));
    }
    match reply::render::<Readback>(&done.ctx, &done.accepted, done.output).await? {
        Some(thread) => Ok(ThreadAdoptRetryHistoryResponse::Full(Box::new(thread))),
        None => Ok(ThreadAdoptRetryHistoryResponse::Receipt(receipt)),
    }
}

struct Readback;

impl reply::Projection for Readback {
    type Source = Source;
    type Input = Accepted;
    type Subject = orch_db::queries::threads::ThreadRow;
    type Value = content_sources::Prepared;
    type Reply = Thread;
    const READ_SOURCE: Source = Source::RetainedThread;
    fn declaration(_: &Accepted) -> &'static Declaration<Source> {
        &rows::RETRY
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = input.written.thread;
            let row = orch_db::queries::threads::thread(conn, thread)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let source = retry::select(conn, thread)
                .await
                .map_err(|error| match error {
                    AdoptError::Internal(error) => ApiError::engine(error),
                    _ => ApiError::not_found(),
                })?;
            let facts = source::retry_history_facts(conn, &source)
                .await
                .map_err(ApiError::engine)?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            deadline(ctx, input.until)?;
            crate::middleware::authorization::require(
                conn,
                principal,
                subject.node_id,
                crate::middleware::authorization::VIEW,
                Act::Use,
                Policy::default(),
            )
            .await
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
                vec![native::Facts::collection(Source::Header, groups)],
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
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Accepted,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(deadline(ctx, input.until).is_ok()
                && old.subject.node_id == fresh.subject.node_id
                && old.value.preserves(&fresh.value))
        })
    }
}
