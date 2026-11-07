//! The native same-environment fork consumes the shared source and acknowledges its actual reserved identities.
use super::preparation::{engine_error, error, source_error};
use super::*;
use crate::key_operations::{
    native::{self, effects},
    publication, targets,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    fork_sources::{self as rows, PlanSource as Role},
    forks::{ForkNowReceipt, ForkNowResponse},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_engine::fork_plan::{
    PreparedFork,
    operation::{self as source, Immediate as Intent},
};
use std::sync::Arc;
const ROW: &Declaration<Role> = &rows::NOW;
pub(super) struct Now;
pub(super) struct Input {
    native: PreparedFork,
    source: source::Input,
}
pub(super) struct Fetched {
    native: PreparedFork,
    draft: Draft,
    origin: source::Origin,
}
pub(super) struct Value {
    fetched: Arc<Fetched>,
    draft: Draft,
}
pub(super) struct Accepted {
    job: super::job_reply::Outcome,
    receipt: ForkNowReceipt,
}
async fn primary(conn: &mut Conn, id: Id) -> ApiResult<native::Facts<Role>> {
    let target = targets::node(conn, id).await?;
    if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
        return Err(ApiError::not_found());
    }
    Ok(native::Facts::one(Role::Thread, target.object.kind, target))
}
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkNowBody,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let plan = effects::prepare::<Now>(ctx, conn, caller, query, body).await?;
        let fetched = effects::initial_value::<Now>(ctx)?;
        plan.with_origin(ctx, conn, caller, fetched.origin.captured())
            .await
    })
}
impl effects::Initial for Now {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(_input: &Input) -> &'static Declaration<Role> {
        ROW
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkNowBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let known = native::bind_known(ctx, ROW, &[primary(conn, id).await?])?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let native = match ctx
                .app
                .engine
                .prepare_fork_in(
                    conn,
                    principal.caller(),
                    id,
                    &body.request(),
                    orch_engine::fork_plan::Reach::Seen,
                    None,
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            let source = source::Input::selected(conn, &native)
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
                .assemble_prepared_fork_with_source(&mut conn, &input.native, Some(&preparation))
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
impl effects::Effect for Now {
    type Source = Role;
    type Query = ();
    type Body = ForkNowBody;
    type Value = Value;
    type Output = Accepted;
    fn declaration(_value: &Value) -> &'static Declaration<Role> {
        ROW
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkNowBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Now>(ctx)?;
            if fetched.origin.input().thread() != ctx.id("id")?
                || fetched.origin.input().request() != &body.request()
            {
                return Err(ApiError::internal(
                    "the native immediate fork has another request",
                ));
            }
            source::lock(conn, fetched.origin.input())
                .await
                .map_err(source_error)?;
            let facts = source::facts::<Intent>(conn, fetched.origin.input())
                .await
                .map_err(source_error)?;
            let plan = native::bind_required(ctx, ROW, &facts)?
                .with_origin(ctx, conn, caller, fetched.origin.captured())
                .await?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let current = match ctx
                .app
                .engine
                .prepare_fork_in(
                    conn,
                    principal.caller(),
                    ctx.id("id")?,
                    &body.request(),
                    orch_engine::fork_plan::Reach::Seen,
                    None,
                    None,
                )
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if !fetched.native.preserves(&current) {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native immediate fork source changed",
                ));
            }
            let mut draft = fetched.draft.clone();
            complete(conn, &principal, &mut draft, current.source().topic_id).await?;
            Ok((Value { fetched, draft }, facts))
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
impl effects::ApplyOwned for Now {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let plan = match ctx
                .app
                .engine
                .keep_fork_plan_on(&mut tx, principal.caller(), value.draft.clone(), || {
                    value
                        .fetched
                        .origin
                        .deadline(ctx.app.clock.as_ref(), principal.caller())?;
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, &mut tx, why).await),
            };
            let body = ForkCreateBody {
                preflight_id: plan.preflight_id,
                options: None,
                notify: None,
            };
            let prepared = match ctx
                .app
                .engine
                .prepare_fork_confirmation(&mut tx, principal.caller(), ctx.id("id")?, &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, &mut tx, why).await),
            };
            let row = ctx
                .app
                .engine
                .commit_fork_confirmation_with_source(
                    tx,
                    principal.caller(),
                    &prepared,
                    value.draft.clone(),
                    &value.fetched.origin,
                    || {
                        deadline.check().map_err(|error| {
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
            let id = row
                .plan
                .get("new_thread")
                .and_then(|id| id.as_str())
                .and_then(|id| id.parse::<Id>().ok())
                .ok_or_else(|| {
                    ApiError::internal("the native immediate fork reserved no Thread")
                })?;
            Ok(effects::Outcome::Applied(Accepted {
                receipt: ForkNowReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    job_id: row.id,
                    thread_id: id,
                },
                job: super::job_reply::Outcome {
                    row,
                    parent: ctx.id("id")?,
                    actor: principal.user(),
                    declaration: ROW,
                    copied: Vec::new(),
                },
            }))
        })
    }
}
pub(super) async fn run(ctx: Ctx, (): (), _body: ForkNowBody) -> ApiResult<ForkNowResponse> {
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    let done = effects::apply_owned::<Now>(&ctx, tx).await?;
    publication::receipt(&done.ctx, &done.output.receipt)?;
    let receipt = done.output.receipt.clone();
    let thread_id = receipt.thread_id;
    match super::job_reply::render(&done.ctx, &done.accepted, done.output.job).await? {
        Some(job) => Ok(ForkNowResponse::Full(Box::new(ForkNowReply {
            job,
            thread_id,
        }))),
        None => Ok(ForkNowResponse::Receipt(receipt)),
    }
}
