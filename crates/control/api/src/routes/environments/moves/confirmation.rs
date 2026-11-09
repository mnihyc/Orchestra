//! Move confirmation consumes one native observation and queues through the same lifecycle source.
use super::operation::Draft;
use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication,
};
use crate::routes::environments::lifecycle as common;
use futures::future::BoxFuture;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{
    environment_sources::lifecycle::{MOVE_CREATE, Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_engine::jobs::environment_move::{
    self,
    preparation::{self, Kind, Origin},
};
use std::sync::Arc;

const CREATION_KIND: &str = "environment_move";
#[derive(serde::Deserialize)]
struct Accepted {
    environment: Id,
}
pub(super) fn binding()
-> crate::middleware::idempotency::Creation<(), ForkCreateBody, api::EnvironmentMoveCreateResponse>
{
    crate::middleware::idempotency::Creation {
        kind: CREATION_KIND,
        identity: |value| {
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION_KIND.into(),
                id: match value {
                    api::EnvironmentMoveCreateResponse::Full(value) => value.id,
                    api::EnvironmentMoveCreateResponse::Receipt(value) => value.job_id,
                },
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                if created.kind != CREATION_KIND {
                    return Err(ApiError::internal(
                        "the accepted environment move kind changed",
                    ));
                }
                let accepted: Accepted =
                    serde_json::from_value(created.data).map_err(ApiError::internal)?;
                if ctx.id("id")? != accepted.environment {
                    return Err(ApiError::not_found());
                }
                let receipt = api::EnvironmentMoveCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    environment_id: accepted.environment,
                    job_id: created.id,
                };
                publication::receipt(&ctx, &receipt)?;
                match reply::render_completed_replay::<Content>(
                    &ctx,
                    ctx.user()?.id(),
                    Readback::Replay {
                        job: created.id,
                        environment: accepted.environment,
                    },
                )
                .await
                {
                    Ok(Some(job)) => Ok(api::EnvironmentMoveCreateResponse::full(job)),
                    Ok(None) => Ok(api::EnvironmentMoveCreateResponse::Receipt(receipt)),
                    Err(error)
                        if matches!(
                            error.code(),
                            orch_api_contract::common::ErrorCode::NotFound
                                | orch_api_contract::common::ErrorCode::AuthzDenied
                        ) =>
                    {
                        Ok(api::EnvironmentMoveCreateResponse::Receipt(receipt))
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(super) struct Confirm;
pub(super) struct Input {
    environment: Id,
    body: ForkCreateBody,
    original: PreflightRow,
}
pub(super) struct Observed {
    environment: Id,
    body: ForkCreateBody,
    original: PreflightRow,
    origin: Origin,
    draft: Draft,
}
impl common::Observed for Observed {
    fn origin(&self) -> &Origin {
        &self.origin
    }
}
#[derive(Clone)]
enum Queue {
    Hosted(Box<environment_move::PreparedQueue>),
    Portable(Box<machine_move::PreparedQueue>),
}
pub(super) struct Value {
    observed: Arc<Observed>,
    queue: Queue,
}
pub(super) struct Queued {
    id: Id,
    environment: Id,
    origin: Origin,
    first: orch_api_contract::workspace::Job,
}
impl effects::Initial for Confirm {
    type Input = Input;
    type Fetched = Observed;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &MOVE_CREATE.declaration
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let original = jobs::forks::preflight(conn, body.preflight_id)
                .await?
                .filter(|row| {
                    row.kind == "environment_move"
                        && row.environment_id == Some(environment)
                        && row.created_by == caller.user()
                })
                .ok_or_else(ApiError::not_found)?;
            if original.expires_at <= ctx.app.clock.now() {
                return Err(refused(updates::UpdateError::Expired));
            }
            let facts = common::objects(
                conn,
                Source::Environments,
                [(ObjectKind::Environment, environment)],
            )
            .await?;
            Ok((
                Input {
                    environment,
                    body,
                    original,
                },
                vec![facts],
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { common::ordinary(conn, p, [input.environment]).await })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Observed>> {
        Box::pin(async move {
            let mut tx = ctx.app.db.begin().await?;
            let scope = common::source(
                ctx,
                &mut tx,
                caller,
                Kind::MoveCreate,
                &serde_json::to_value(&input.body).map_err(ApiError::internal)?,
                &[input.environment],
                Some(&input.original),
            )
            .await?;
            let request: api::EnvironmentMovePreflightBody =
                serde_json::from_value(input.original.plan["request"].clone())
                    .map_err(ApiError::internal)?;
            let hosted: Option<MoveRecipe> = input
                .original
                .plan
                .get("recipe")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(ApiError::internal)?;
            let portable: Option<machine_move::Recipe> = input
                .original
                .plan
                .get("portable")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(ApiError::internal)?;
            let files = match (&hosted, &portable) {
                (Some(recipe), None) => &recipe.files,
                (None, Some(recipe)) => &recipe.files,
                _ => return Err(ApiError::not_found()),
            };
            let scope = scope.with_saved_files(files);
            tx.commit().await.map_err(DbError::from)?;
            let current = scope
                .current(&ctx.app.engine)
                .await
                .map_err(common::error)?;
            let mut tx = ctx.app.db.begin().await?;
            let p = authorization::load_principal(&mut tx, current, ctx.app.clock.time()).await?;
            let draft = preparation::scope(&scope, async {
                if let Some(recipe) = portable {
                    tx.rollback().await.map_err(DbError::from)?;
                    let draft = ctx
                        .app
                        .engine
                        .draft_machine_move(
                            &p,
                            input.environment,
                            &request,
                            input.body.options.as_ref(),
                        )
                        .await
                        .map_err(refused)?;
                    if !recipe.same_choice(&draft.recipe) {
                        return Err(stale());
                    }
                    Ok(Draft::Portable(Box::new(draft)))
                } else {
                    let recipe = hosted.ok_or_else(ApiError::not_found)?;
                    let draft = ctx
                        .app
                        .engine
                        .draft_move(
                            &mut tx,
                            &p,
                            input.environment,
                            &request,
                            input.body.options.as_ref(),
                        )
                        .await
                        .map_err(refused)?;
                    tx.rollback().await.map_err(DbError::from)?;
                    if draft.recipe.replacement.old != recipe.replacement.old {
                        return Err(stale());
                    }
                    Ok(Draft::Hosted(Box::new(draft)))
                }
            })
            .await?;
            scope
                .current(&ctx.app.engine)
                .await
                .map_err(common::error)?;
            Ok(Observed {
                environment: input.environment,
                body: input.body,
                original: input.original,
                origin: scope.origin().await,
                draft,
            })
        })
    }
}
fn stale() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        "The source, destination or promised data changed; review the move again.",
    )
}
impl effects::Effect for Confirm {
    type Source = Source;
    type Query = ();
    type Body = ForkCreateBody;
    type Value = Value;
    type Output = Queued;
    fn declaration(_: &Value) -> &'static Declaration<Source> {
        &MOVE_CREATE.declaration
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        _: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let observed = effects::initial_value::<Self>(ctx)?;
            let current = jobs::changes::lock_plan(conn, observed.original.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if current.plan != observed.original.plan || current.expires_at <= ctx.app.clock.now() {
                return Err(stale());
            }
            let check = observed
                .origin
                .checkpoint(conn, ctx.app.clock.as_ref())
                .await
                .map_err(common::error)?;
            let p =
                authorization::load_principal(conn, check.caller().clone(), ctx.app.clock.time())
                    .await?;
            let queue = match &observed.draft {
                Draft::Hosted(d) => Queue::Hosted(Box::new(
                    environment_move::prepare_queue(conn, d.as_ref().clone())
                        .await
                        .map_err(refused)?,
                )),
                Draft::Portable(d) => Queue::Portable(Box::new(
                    machine_move::prepare_queue(conn, &p, d.as_ref().clone(), ctx.now)
                        .await
                        .map_err(refused)?,
                )),
            };
            common::current(ctx, conn, &observed.origin).await?;
            let facts = observed
                .origin
                .facts(conn, true)
                .await
                .map_err(common::source_error)?;
            Ok((Value { observed, queue }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            value
                .observed
                .origin
                .require(conn, p)
                .await
                .map_err(common::error)
        })
    }
}
impl effects::Apply for Confirm {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Queued>>> {
        Box::pin(async move {
            let observed = &value.observed;
            let operation = move_operation(
                conn,
                p,
                observed.environment,
                observed.original.id,
                observed.body.notify.unwrap_or(false),
            )
            .await?;
            common::current(ctx, conn, &observed.origin).await?;
            let deadline = before.deadline();
            before.check()?;
            let row = orch_engine::notifications::operation_scope(Some(operation.id), async {
                match &value.queue {
                    Queue::Hosted(queue) => {
                        environment_move::queue_prepared(
                            conn,
                            queue.as_ref().clone(),
                            None,
                            ctx.now,
                            || Ok(()),
                        )
                        .await
                    }
                    Queue::Portable(queue) => {
                        machine_move::queue_prepared(
                            conn,
                            queue.as_ref().clone(),
                            None,
                            ctx.now,
                            || Ok(()),
                        )
                        .await
                    }
                }
            })
            .await
            .map_err(refused)?;
            ctx.app
                .engine
                .retain_environment_move_source(conn, &row, &observed.origin)
                .await
                .map_err(common::error)?;
            orch_engine::notifications::attach_operation(conn, &[row.id], &operation, ctx.now)
                .await
                .map_err(ApiError::internal)?;
            orch_engine::notifications::notify_operation(conn, &operation, "running", ctx.now)
                .await
                .map_err(ApiError::internal)?;
            jobs::forks::forget_preflight(conn, observed.original.id).await?;
            let first = super::super::jobs::view(conn, &row).await?;
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(ctx.user()?.id()),
                CREATION_KIND,
                row.id,
                serde_json::json!({"environment":observed.environment}),
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Queued {
                id: row.id,
                environment: observed.environment,
                origin: observed.origin.clone(),
                first,
            }))
        })
    }
}
enum Readback {
    First(Box<Queued>),
    Replay { job: Id, environment: Id },
}
impl Readback {
    fn job(&self) -> Id {
        match self {
            Self::First(value) => value.id,
            Self::Replay { job, .. } => *job,
        }
    }
    fn environment(&self) -> Id {
        match self {
            Self::First(value) => value.environment,
            Self::Replay { environment, .. } => *environment,
        }
    }
    async fn required(&self, conn: &mut Conn) -> ApiResult<Vec<native::Facts<Source>>> {
        match self {
            Self::First(value) => value
                .origin
                .facts(conn, true)
                .await
                .map_err(common::source_error),
            Self::Replay { .. } => Ok(Vec::new()),
        }
    }
}
struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Readback;
    type Subject = jobs::JobRow;
    type Value = std::collections::HashMap<Id, String>;
    type Reply = orch_api_contract::workspace::Job;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Readback) -> &'static Declaration<Source> {
        &MOVE_CREATE.declaration
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<(jobs::JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let account = ctx.user()?.id();
            let row = jobs::job(conn, input.job())
                .await?
                .filter(|r| {
                    r.kind == jobs::kind::ENVIRONMENT_MOVE
                        && r.environment_id == Some(input.environment())
                        && r.created_by == Some(account)
                })
                .ok_or_else(ApiError::not_found)?;
            let mut facts = input.required(conn).await?;
            // A new queued response has no waiting-machine name yet. Suppress any later cached
            // label instead of lending it a resource the actual first answer did not read.
            facts.push(
                common::objects(
                    conn,
                    Source::Content,
                    [(ObjectKind::Environment, input.environment())],
                )
                .await?,
            );
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Readback,
        _: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            super::super::reading::decide(conn, p, input.environment())
                .await
                .map(|_| ())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        _: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok((Default::default(), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            super::super::jobs::view_with_machine_sources(
                conn,
                &opened.subject,
                Some(&opened.value),
            )
            .await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.environment_id == fresh.subject.environment_id
                && old.subject.created_by == fresh.subject.created_by)
        })
    }
}
pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _: ForkCreateBody,
) -> ApiResult<api::EnvironmentMoveCreateResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Confirm>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    let receipt = api::EnvironmentMoveCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done.output.environment,
        job_id: done.output.id,
    };
    publication::receipt(&done.ctx, &receipt)?;
    if !native::modern(&done.accepted) {
        return Ok(api::EnvironmentMoveCreateResponse::full(done.output.first));
    }
    Ok(
        match reply::render::<Content>(
            &done.ctx,
            &done.accepted,
            Readback::First(Box::new(done.output)),
        )
        .await?
        {
            Some(job) => api::EnvironmentMoveCreateResponse::full(job),
            None => api::EnvironmentMoveCreateResponse::Receipt(receipt),
        },
    )
}
