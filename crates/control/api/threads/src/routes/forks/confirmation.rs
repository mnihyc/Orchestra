//! Confirmation observes its kept native plan once and commits the selected native writer.

use super::preparation::engine_error;

use super::preparation::error;

use super::preparation::source_error;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::publication;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_api_contract::fork_sources::PlanSource as Role;

use orch_api_contract::forks::ForkCreateReceipt;

use orch_api_contract::forks::ForkCreateResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_engine::fork_plan::operation as source;

use orch_engine::jobs::fork::Confirmation;

use std::sync::Arc;
pub(super) struct Create<I: source::Intent = source::Confirm>(std::marker::PhantomData<I>);

pub(super) struct Input {
    native: Confirmation,
    source: source::Input,
}

pub(super) struct Fetched {
    native: Confirmation,
    draft: Draft,
    origin: source::Origin,
}

pub(super) struct Value {
    fetched: Arc<Fetched>,
    native: Confirmation,
    draft: Draft,
}

pub(super) struct Accepted {
    job: super::job_reply::Outcome,
    receipt: ForkCreateReceipt,
}

pub(super) fn declaration(result: ForkResult) -> &'static Declaration<Role> {
    declaration_for::<source::Confirm>(result)
}

fn declaration_for<I: source::Intent>(result: ForkResult) -> &'static Declaration<Role> {
    if result == ForkResult::New {
        I::ROW.fork.unwrap_or(I::ROW.default)
    } else {
        I::ROW.switch.unwrap_or(I::ROW.default)
    }
}

async fn primary(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Role>> {
    let target = targets::node(conn, id).await?;
    if target.object.kind
        != orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Thread)
    {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(Role::Thread, target.object.kind, target))
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkCreateBody,
) -> crate::registry::OperationFuture<'a> {
    prepare_for::<source::Confirm>(ctx, conn, caller, query, body)
}

pub(super) fn prepare_move<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkCreateBody,
) -> crate::registry::OperationFuture<'a> {
    prepare_for::<source::MoveConfirm>(ctx, conn, caller, query, body)
}

fn prepare_for<'a, I: source::Intent>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkCreateBody,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let plan = effects::prepare::<Create<I>>(ctx, conn, caller, query, body).await?;
        let fetched = effects::initial_value::<Create<I>>(ctx)?;
        plan.with_origin(ctx, conn, caller, fetched.origin.captured())
            .await
    })
}

impl<I: source::Intent> effects::Initial for Create<I> {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(input: &Input) -> &'static Declaration<Role> {
        declaration_for::<I>(input.native.selected().request().result)
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            // Both native variants share the same actual source admission. The kept native
            // plan, not caller JSON, selects the final variant before any supplier is run.
            let result = match ctx
                .app
                .engine
                .fork_confirmation_result(conn, caller, id, &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            let (request, purpose) = ctx
                .app
                .engine
                .fork_confirmation_input(conn, caller, id, &body)
                .await
                .map_err(|error| {
                    engine_error(match error {
                        PlanError::Internal(error) => error,
                        other => orch_engine::EngineError::unreadable(other.to_string()),
                    })
                })?;
            if I::KIND.is_move() != purpose.is_some() {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "this preflight has another native purpose",
                ));
            }
            let mut prefix = vec![primary(conn, id).await?];
            prefix.extend(
                source::move_prefix::<I>(conn, id, &request)
                    .await
                    .map_err(engine_error)?,
            );
            let known = native::bind_known(ctx, declaration_for::<I>(result), &prefix)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let native = match ctx
                .app
                .engine
                .select_fork_confirmation(conn, principal.caller(), id, &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if I::KIND.is_move() != native.moving().is_some() {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "this confirmation has another native preflight purpose",
                ));
            }
            let source = source::Input::selected_for::<I>(conn, native.selected())
                .await
                .map_err(engine_error)?;
            if native.moving() != source.moving() {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the reviewed original Thread deletion changed",
                ));
            }
            let facts = source::facts::<I>(conn, &source)
                .await
                .map_err(source_error)?;
            Ok((Input { native, source }, facts))
        })
    }
    fn require_initial<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Some(review) = input.source.moving() {
                orch_engine::fork_plan::moving::require::<I>(conn, principal, review)
                    .await
                    .map_err(engine_error)?;
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let (credential, until) =
                crate::routes::threads::adopt::operation::http_origin(ctx).await?;
            let origin = source::Origin::capture::<I>(caller, input.source, credential, until)
                .map_err(engine_error)?;
            let preparation = source::Preparation::new(origin);
            let mut conn = ctx.app.db.acquire().await?;
            let draft = match ctx
                .app
                .engine
                .assemble_prepared_fork_with_source(
                    &mut conn,
                    input.native.selected(),
                    Some(&preparation),
                )
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, &mut conn, why).await),
            };
            Ok(Fetched {
                native: input.native,
                draft,
                origin: preparation.retained().await,
            })
        })
    }
}

impl<I: source::Intent> effects::Effect for Create<I> {
    type Source = Role;
    type Query = ();
    type Body = ForkCreateBody;
    type Value = Value;
    type Output = Accepted;
    fn declaration(value: &Value) -> &'static Declaration<Role> {
        declaration_for::<I>(value.native.selected().request().result)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Create<I>>(ctx)?;
            if fetched.origin.input().thread() != ctx.id("id")?
                || fetched.native.preflight_id() != body.preflight_id
            {
                return Err(ApiError::internal(
                    "the native confirmation has another kept request",
                ));
            }
            source::lock(conn, fetched.origin.input())
                .await
                .map_err(source_error)?;
            let facts = source::facts::<I>(conn, fetched.origin.input())
                .await
                .map_err(source_error)?;
            let plan = native::bind_required(
                ctx,
                declaration_for::<I>(fetched.native.selected().request().result),
                &facts,
            )?
            .with_origin(ctx, conn, caller, fetched.origin.captured())
            .await?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let native = match ctx
                .app
                .engine
                .prepare_fork_confirmation(conn, principal.caller(), ctx.id("id")?, &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if !fetched.native.preserves(&native)
                || source::Input::selected_for::<I>(conn, native.selected())
                    .await
                    .map_err(engine_error)?
                    != *fetched.origin.input()
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native confirmation source changed",
                ));
            }
            let mut draft = fetched.draft.clone();
            complete(
                conn,
                &principal,
                &mut draft,
                native.selected().source().topic_id,
            )
            .await?;
            Ok((
                Value {
                    fetched,
                    native,
                    draft,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if let Some(review) = value.fetched.origin.input().moving() {
                orch_engine::fork_plan::moving::require::<I>(conn, principal, review)
                    .await
                    .map_err(engine_error)?;
            }
            value
                .fetched
                .origin
                .deadline(ctx.app.clock.as_ref(), principal.caller())
                .map_err(engine_error)
        })
    }
}

impl<I: source::Intent> effects::ApplyOwned for Create<I> {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let row = ctx
                .app
                .engine
                .commit_fork_confirmation_with_source(
                    tx,
                    principal.caller(),
                    &value.native,
                    value.draft.clone(),
                    &value.fetched.origin,
                    || {
                        value
                            .fetched
                            .origin
                            .deadline(ctx.app.clock.as_ref(), principal.caller())?;
                        before.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                    || {
                        deadline.check().map_err(|error| {
                            orch_engine::EngineError::BeforeEffect(Box::new(error))
                        })
                    },
                )
                .await;
            let row = match row {
                Ok(value) => value,
                Err(why) => {
                    let mut conn = ctx.app.db.acquire().await?;
                    return Err(error(ctx, &mut conn, why).await);
                }
            };
            let copied = match &value.draft.request.access {
                Some(orch_api_contract::access::ThreadAccess::As(page))
                    if page.source != ctx.id("id")? =>
                {
                    vec![page.source]
                }
                _ => Vec::new(),
            };
            Ok(effects::Outcome::Applied(Accepted {
                receipt: ForkCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    job_id: row.id,
                },
                job: super::job_reply::Outcome {
                    row,
                    parent: ctx.id("id")?,
                    actor: principal.user(),
                    declaration: declaration_for::<I>(value.draft.plan.result),
                    copied: Some(copied),
                },
            }))
        })
    }
}

async fn run_for<I: source::Intent>(ctx: Ctx) -> ApiResult<ForkCreateResponse> {
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    let done = effects::apply_owned::<Create<I>>(&ctx, tx).await?;
    publication::receipt(&done.ctx, &done.output.receipt)?;
    let receipt = done.output.receipt.clone();
    if I::KIND.is_move() {
        return Ok(ForkCreateResponse::Receipt(receipt));
    }
    match super::job_reply::render(&done.ctx, &done.accepted, done.output.job).await? {
        Some(job) => Ok(ForkCreateResponse::Full(Box::new(job))),
        None => Ok(ForkCreateResponse::Receipt(receipt)),
    }
}

pub(super) async fn run(ctx: Ctx, (): (), _: ForkCreateBody) -> ApiResult<ForkCreateResponse> {
    run_for::<source::Confirm>(ctx).await
}

pub(super) async fn run_move(ctx: Ctx, (): (), _: ForkCreateBody) -> ApiResult<ForkCreateResponse> {
    run_for::<source::MoveConfirm>(ctx).await
}
