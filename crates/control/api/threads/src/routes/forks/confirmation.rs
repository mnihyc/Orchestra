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

use orch_api_contract::fork_sources as rows;

use orch_api_contract::fork_sources::PlanSource as Role;

use orch_api_contract::forks::ForkCreateReceipt;

use orch_api_contract::forks::ForkCreateResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_engine::fork_plan::operation as source;

use orch_engine::fork_plan::operation::Confirm as Intent;

use orch_engine::jobs::fork::Confirmation;

use std::sync::Arc;
pub(super) struct Create;

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
    if result == ForkResult::New {
        &rows::CONFIRM_FORK
    } else {
        &rows::CONFIRM_SWITCH
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
    Box::pin(async move {
        let plan = effects::prepare::<Create>(ctx, conn, caller, query, body).await?;
        let fetched = effects::initial_value::<Create>(ctx)?;
        plan.with_origin(ctx, conn, caller, fetched.origin.captured())
            .await
    })
}

impl effects::Initial for Create {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(input: &Input) -> &'static Declaration<Role> {
        declaration(input.native.selected().request().result)
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
            let known = native::bind_known(ctx, declaration(result), &[primary(conn, id).await?])?;
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
            let source = source::Input::selected(conn, native.selected())
                .await
                .map_err(engine_error)?;
            let facts = source::facts::<Intent>(conn, &source)
                .await
                .map_err(source_error)?;
            Ok((Input { native, source }, facts))
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
            let origin = source::Origin::capture::<Intent>(caller, input.source, credential, until)
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

impl effects::Effect for Create {
    type Source = Role;
    type Query = ();
    type Body = ForkCreateBody;
    type Value = Value;
    type Output = Accepted;
    fn declaration(value: &Value) -> &'static Declaration<Role> {
        declaration(value.native.selected().request().result)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Create>(ctx)?;
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
            let facts = source::facts::<Intent>(conn, fetched.origin.input())
                .await
                .map_err(source_error)?;
            let plan = native::bind_required(
                ctx,
                declaration(fetched.native.selected().request().result),
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
                || source::Input::selected(conn, native.selected())
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
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            value
                .fetched
                .origin
                .deadline(ctx.app.clock.as_ref(), principal.caller())
                .map_err(engine_error)
        })
    }
}

impl effects::ApplyOwned for Create {
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
                    declaration: declaration(value.draft.plan.result),
                    copied: Some(copied),
                },
            }))
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), _body: ForkCreateBody) -> ApiResult<ForkCreateResponse> {
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    let done = effects::apply_owned::<Create>(&ctx, tx).await?;
    publication::receipt(&done.ctx, &done.output.receipt)?;
    let receipt = done.output.receipt.clone();
    match super::job_reply::render(&done.ctx, &done.accepted, done.output.job).await? {
        Some(job) => Ok(ForkCreateResponse::Full(Box::new(job))),
        None => Ok(ForkCreateResponse::Receipt(receipt)),
    }
}
