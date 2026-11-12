//! Mixed recovery confirmation prepares every native member before its shared effect cut.

use super::super::lifecycle as common;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::middleware::transaction_guard;

use futures::future::BoxFuture;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment_sources::lifecycle::RECOVER_CREATE;

use orch_api_contract::environment_sources::lifecycle::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_engine::fork_plan::Draft as ForkDraft;

use orch_engine::jobs::batch::LifecycleConfirmation;

use orch_engine::jobs::environment_move;

use orch_engine::jobs::environment_move::preparation;

use orch_engine::jobs::environment_move::preparation::Kind;

use orch_engine::jobs::environment_move::preparation::Origin;

use orch_engine::jobs::machine_move;

use std::sync::Arc;
const CREATION_KIND: &str = "environment_recovery_batch";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct AcceptedJob {
    id: Id,
    kind: String,
    environment: Option<Id>,
    node: Option<Id>,
}

impl From<&jobs::JobRow> for AcceptedJob {
    fn from(row: &jobs::JobRow) -> Self {
        Self {
            id: row.id,
            kind: row.kind.clone(),
            environment: row.environment_id,
            node: row.node_id,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Accepted {
    jobs: Vec<AcceptedJob>,
}

pub(super) fn binding() -> crate::middleware::idempotency::Creation<
    (),
    ForkCreateBody,
    api::EnvironmentRecoverCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: CREATION_KIND,
        identity: |value| {
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION_KIND.into(),
                id: match value {
                    api::EnvironmentRecoverCreateResponse::Full(value) => value.batch_id,
                    api::EnvironmentRecoverCreateResponse::Receipt(value) => value.batch_id,
                },
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                if created.kind != CREATION_KIND {
                    return Err(ApiError::internal(
                        "the accepted native recovery kind changed",
                    ));
                }
                let accepted: Accepted =
                    serde_json::from_value(created.data).map_err(ApiError::internal)?;
                let receipt = api::EnvironmentRecoverCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    batch_id: created.id,
                    job_ids: accepted.jobs.iter().map(|row| row.id).collect(),
                };
                publication::receipt(&ctx, &receipt)?;
                match reply::render_completed_replay::<Content>(
                    &ctx,
                    ctx.user()?.id(),
                    Readback::Replay {
                        batch: created.id,
                        jobs: accepted.jobs,
                    },
                )
                .await
                {
                    Ok(Some(batch)) => Ok(api::EnvironmentRecoverCreateResponse::full(batch)),
                    Ok(None) => Ok(api::EnvironmentRecoverCreateResponse::Receipt(receipt)),
                    Err(error)
                        if matches!(
                            error.code(),
                            orch_api_contract::common::ErrorCode::NotFound
                                | orch_api_contract::common::ErrorCode::AuthzDenied
                        ) =>
                    {
                        Ok(api::EnvironmentRecoverCreateResponse::Receipt(receipt))
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(super) struct Confirm;

pub(super) struct Input {
    original: jobs::forks::PreflightRow,
    stored: Stored,
    body: ForkCreateBody,
    batch: Id,
}

#[derive(Clone)]
enum Draft {
    Recreate(Box<recovery::Recipe>),
    Hosted(Box<environment_move::MoveDraft>),
    Portable(Box<machine_move::Draft>),
}

struct Switches {
    selected: LifecycleConfirmation,
    drafts: Vec<Option<ForkDraft>>,
}

pub(super) struct Observed {
    original: jobs::forks::PreflightRow,
    stored: Stored,
    body: ForkCreateBody,
    batch: Id,
    origin: Origin,
    members: Vec<(Id, Draft)>,
    switches: Option<Switches>,
}

impl common::Observed for Observed {
    fn origin(&self) -> &Origin {
        &self.origin
    }
}

#[derive(Clone)]
enum Queue {
    Recreate(Box<recovery::PreparedQueue>),
    Hosted(Box<environment_move::PreparedQueue>),
    Portable(Box<machine_move::PreparedQueue>),
}

pub(super) struct Value {
    observed: Arc<Observed>,
    members: Vec<(Id, Queue)>,
    switches: Option<LifecycleConfirmation>,
}

pub(super) struct Queued {
    batch: Id,
    jobs: Vec<jobs::JobRow>,
    origin: Origin,
    first: Option<EnvironmentBatch>,
}

fn failure(error: orch_engine::fork_plan::PlanError) -> ApiError {
    match error {
        orch_engine::fork_plan::PlanError::Internal(orch_engine::EngineError::BeforeEffect(
            error,
        )) => common::error(orch_engine::EngineError::BeforeEffect(error)),
        other => ApiError::new(
            orch_api_contract::common::ErrorCode::PlanBlocked,
            other.to_string(),
        ),
    }
}

impl effects::Initial for Confirm {
    type Input = Input;
    type Fetched = Observed;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &RECOVER_CREATE.declaration
    }
    fn authorize<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let original = jobs::forks::preflight(conn, body.preflight_id)
                .await?
                .filter(|row| row.kind == "recover_batch" && row.created_by == caller.user())
                .ok_or_else(ApiError::not_found)?;
            let stored: Stored =
                serde_json::from_value(original.plan.clone()).map_err(ApiError::internal)?;
            let facts = common::objects(
                conn,
                Source::Environments,
                stored
                    .affected
                    .iter()
                    .map(|id| (ObjectKind::Environment, *id)),
            )
            .await?;
            Ok((
                Input {
                    original,
                    stored,
                    body,
                    batch: new_id(),
                },
                vec![facts],
            ))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            if input.original.expires_at <= ctx.app.clock.now() {
                return Err(super::super::updates::refused(UpdateError::Expired));
            }
            common::ordinary(conn, p, input.stored.affected.iter().copied()).await?;
            // A blocked review has no work to prepare or retain as a native execution source.
            if input.stored.affected.is_empty() {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::PlanBlocked,
                    "No recovery member can run",
                ));
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Observed>> {
        Box::pin(async move {
            let mut tx = ctx.app.db.begin().await?;
            let mut scope = common::source(
                ctx,
                &mut tx,
                caller,
                Kind::RecoverCreate,
                &serde_json::to_value(&input.body).map_err(ApiError::internal)?,
                &input.stored.affected,
                Some(&input.original),
            )
            .await?;
            for member in &input.stored.members {
                match member {
                    Member::Move { recipe, .. } => scope = scope.with_saved_files(&recipe.files),
                    Member::Portable { recipe, .. } => {
                        scope = scope.with_saved_files(&recipe.files)
                    }
                    Member::Recreate { .. } => {}
                }
            }
            tx.commit().await.map_err(DbError::from)?;
            let mut members = Vec::new();
            for member in &input.stored.members {
                let environment = match member {
                    Member::Recreate { recipe } => recipe.environment,
                    Member::Move { recipe, .. } => recipe.replacement.target.id(),
                    Member::Portable { recipe, .. } => recipe.environment,
                };
                let caller = scope
                    .current(&ctx.app.engine)
                    .await
                    .map_err(common::error)?;
                let mut tx = ctx.app.db.begin().await?;
                let p =
                    authorization::load_principal(&mut tx, caller, ctx.app.clock.time()).await?;
                let named = match member {
                    Member::Recreate { recipe } => Some(recipe.host),
                    Member::Move { recipe, .. } => Some(recipe.destination),
                    Member::Portable { .. } => None,
                };
                orch_engine::operation_access::preview_recovery(&mut tx, &p, environment, named)
                    .await
                    .map_err(super::super::updates::refused)?;
                let draft = preparation::scope(&scope, async {
                    match member {
                        Member::Recreate { recipe } => {
                            let prepared = ctx
                                .app
                                .engine
                                .prepare_recovery(&mut tx, &p, environment, Some(recipe.host))
                                .await
                                .map_err(super::super::updates::refused)?;
                            tx.rollback().await.map_err(DbError::from)?;
                            let draft = ctx
                                .app
                                .engine
                                .measure_recovery(prepared)
                                .await
                                .map_err(super::super::updates::refused)?;
                            if draft.recipe != **recipe {
                                return Err(stale());
                            }
                            Ok(Draft::Recreate(recipe.clone()))
                        }
                        Member::Move { recipe, request } => {
                            let draft = ctx
                                .app
                                .engine
                                .draft_move(
                                    &mut tx,
                                    &p,
                                    environment,
                                    request,
                                    input.body.options.as_ref(),
                                )
                                .await
                                .map_err(super::super::updates::refused)?;
                            tx.rollback().await.map_err(DbError::from)?;
                            if draft.recipe.replacement.old != recipe.replacement.old
                                || draft.recipe.destination != recipe.destination
                                || draft.recipe.manifest != recipe.manifest
                            {
                                return Err(stale());
                            }
                            Ok(Draft::Hosted(Box::new(draft)))
                        }
                        Member::Portable { recipe, request } => {
                            tx.rollback().await.map_err(DbError::from)?;
                            let draft = ctx
                                .app
                                .engine
                                .draft_machine_move(
                                    &p,
                                    environment,
                                    request,
                                    input.body.options.as_ref(),
                                )
                                .await
                                .map_err(super::super::updates::refused)?;
                            if !recipe.same_choice(&draft.recipe) {
                                return Err(stale());
                            }
                            Ok(Draft::Portable(Box::new(draft)))
                        }
                    }
                })
                .await?;
                members.push((environment, draft));
            }
            let switches = if let Some(stored) = &input.stored.switches {
                let caller = scope
                    .current(&ctx.app.engine)
                    .await
                    .map_err(common::error)?;
                let mut tx = ctx.app.db.begin().await?;
                let selected = preparation::scope(
                    &scope,
                    ctx.app.engine.select_lifecycle_confirmation(
                        &mut tx,
                        &caller,
                        stored,
                        &input.body,
                        input.batch,
                        false,
                    ),
                )
                .await
                .map_err(failure)?;
                let drafts = preparation::scope(
                    &scope,
                    ctx.app
                        .engine
                        .assemble_lifecycle_confirmation(&mut tx, &selected),
                )
                .await
                .map_err(failure)?;
                tx.rollback().await.map_err(DbError::from)?;
                Some(Switches { selected, drafts })
            } else {
                None
            };
            scope
                .current(&ctx.app.engine)
                .await
                .map_err(common::error)?;
            Ok(Observed {
                original: input.original,
                stored: input.stored,
                body: input.body,
                batch: input.batch,
                origin: scope.origin().await,
                members,
                switches,
            })
        })
    }
}

impl effects::Effect for Confirm {
    type Source = Source;
    type Query = ();
    type Body = ForkCreateBody;
    type Value = Value;
    type Output = Queued;
    fn declaration(_: &Value) -> &'static Declaration<Source> {
        &RECOVER_CREATE.declaration
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
            let row = jobs::changes::lock_plan(conn, observed.original.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.plan != observed.original.plan || row.expires_at <= ctx.app.clock.now() {
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
            let mut members = Vec::new();
            for (environment, draft) in &observed.members {
                let queue = match draft {
                    Draft::Recreate(recipe) => Queue::Recreate(Box::new(
                        recovery::prepare_queue(conn, recipe)
                            .await
                            .map_err(super::super::updates::refused)?,
                    )),
                    Draft::Hosted(draft) => Queue::Hosted(Box::new(
                        environment_move::prepare_queue(conn, *draft.clone())
                            .await
                            .map_err(super::super::updates::refused)?,
                    )),
                    Draft::Portable(draft) => Queue::Portable(Box::new(
                        machine_move::prepare_queue(conn, &p, *draft.clone(), ctx.now)
                            .await
                            .map_err(super::super::updates::refused)?,
                    )),
                };
                members.push((*environment, queue));
            }
            let switches = match (&observed.stored.switches, &observed.switches) {
                (Some(stored), Some(original)) => {
                    let current = ctx
                        .app
                        .engine
                        .select_lifecycle_confirmation(
                            conn,
                            p.caller(),
                            stored,
                            &observed.body,
                            observed.batch,
                            true,
                        )
                        .await
                        .map_err(failure)?;
                    if !original.selected.preserves(&current) {
                        return Err(stale());
                    }
                    Some(current)
                }
                (None, None) => None,
                _ => return Err(stale()),
            };
            common::current(ctx, conn, &observed.origin).await?;
            let facts = observed
                .origin
                .facts(conn, true)
                .await
                .map_err(common::source_error)?;
            Ok((
                Value {
                    observed,
                    members,
                    switches,
                },
                facts,
            ))
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

impl effects::ApplyOwned for Confirm {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        p: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Queued>>> {
        Box::pin(async move {
            let observed = &value.observed;
            let audience = orch_engine::notifications::operation_audience(
                &mut tx,
                p,
                &observed.stored.affected,
                &[],
            )
            .await
            .map_err(ApiError::internal)?;
            if !orch_db::queries::notifications::operations::matches_preview(
                &mut tx,
                observed.original.id,
                &audience
                    .iter()
                    .map(|person| person.user)
                    .collect::<Vec<_>>(),
            )
            .await?
            {
                return Err(stale());
            }
            let operation = orch_engine::notifications::NotificationOperation {
                id: observed.batch,
                kind: "recover".into(),
                name: String::new(),
                notify: observed.body.notify.unwrap_or(false),
                path: "/resources/environments".into(),
                subject: None,
                recipients: audience,
            };
            common::current(ctx, &mut tx, &observed.origin).await?;
            let deadline = before.deadline();
            before.check()?;
            orch_engine::operation_access::accept(
                &mut tx,
                p,
                &observed.stored.admin_grants,
                &orch_db::audit::Origin::default(),
            )
            .await
            .map_err(super::super::updates::refused)?;
            let mut queued = Vec::new();
            for (_environment, queue) in &value.members {
                deadline.check()?;
                let row = orch_engine::notifications::operation_scope(Some(operation.id), async {
                    match queue {
                        Queue::Recreate(prepared) => {
                            recovery::queue_prepared(
                                &ctx.app.engine,
                                &mut tx,
                                prepared,
                                Some(observed.batch),
                                ctx.now,
                                || Ok(()),
                            )
                            .await
                        }
                        Queue::Hosted(prepared) => {
                            environment_move::queue_prepared(
                                &mut tx,
                                prepared.as_ref().clone(),
                                Some(observed.batch),
                                ctx.now,
                                || Ok(()),
                            )
                            .await
                        }
                        Queue::Portable(prepared) => {
                            machine_move::queue_prepared(
                                &mut tx,
                                prepared.as_ref().clone(),
                                Some(observed.batch),
                                ctx.now,
                                || Ok(()),
                            )
                            .await
                        }
                    }
                })
                .await
                .map_err(super::super::updates::refused)?;
                let prepared = match queue {
                    Queue::Recreate(prepared) => Some(prepared.as_ref()),
                    _ => None,
                };
                ctx.app
                    .engine
                    .retain_recovery_lifecycle_source(&mut tx, row.id, &observed.origin, prepared)
                    .await
                    .map_err(common::error)?;
                queued.push(
                    jobs::job(&mut tx, row.id)
                        .await?
                        .ok_or_else(ApiError::not_found)?,
                );
            }
            if let (Some(original), Some(current)) = (&observed.switches, &value.switches) {
                deadline.check()?;
                let rows = orch_engine::notifications::operation_scope(
                    Some(operation.id),
                    ctx.app.engine.queue_lifecycle_confirmation(
                        &mut tx,
                        p.caller(),
                        &original.selected,
                        current,
                        &original.drafts,
                    ),
                )
                .await;
                match rows {
                    Ok(rows) => {
                        for row in rows {
                            ctx.app
                                .engine
                                .retain_recovery_lifecycle_source(
                                    &mut tx,
                                    row.id,
                                    &observed.origin,
                                    None,
                                )
                                .await
                                .map_err(common::error)?;
                            queued.push(
                                jobs::job(&mut tx, row.id)
                                    .await?
                                    .ok_or_else(ApiError::not_found)?,
                            );
                        }
                    }
                    Err(orch_engine::fork_plan::PlanError::NothingExecutable) => {}
                    Err(error) => return Err(failure(error)),
                }
            }
            if queued.is_empty() {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::PlanBlocked,
                    "No recovery member can run",
                ));
            }
            orch_engine::notifications::attach_operation(
                &mut tx,
                &queued.iter().map(|row| row.id).collect::<Vec<_>>(),
                &operation,
                ctx.now,
            )
            .await
            .map_err(ApiError::internal)?;
            orch_engine::notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
                .await
                .map_err(ApiError::internal)?;
            for environment in &observed.stored.affected {
                let mut entry = orch_db::audit::Entry::new(
                    orch_db::audit::Actor::User(p.user()),
                    orch_db::audit::name!("environment.updated"),
                    "environment",
                    Some(*environment),
                    "Requested reviewed recovery",
                );
                entry.environment_id = Some(*environment);
                entry.history = true;
                entry.r#override = !observed.stored.admin_grants.is_empty();
                entry.details = serde_json::json!({"operation":observed.batch,"admin_grants":observed.stored.admin_grants});
                orch_db::audit::append(
                    &mut tx,
                    &entry,
                    &orch_db::audit::Origin::default(),
                    ctx.now,
                )
                .await?;
            }
            jobs::forks::forget_preflight(&mut tx, observed.original.id).await?;
            let first = if !native::modern(p.caller()) {
                Some(shown(ctx, &mut tx, p, observed.batch, &queued).await?)
            } else {
                None
            };
            orch_db::idempotency::creation::observe_with(
                &mut tx,
                Some(ctx.user()?.id()),
                CREATION_KIND,
                observed.batch,
                serde_json::to_value(Accepted {
                    jobs: queued.iter().map(AcceptedJob::from).collect(),
                })
                .map_err(ApiError::internal)?,
            )
            .await?;
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(Queued {
                batch: observed.batch,
                jobs: queued,
                origin: observed.origin.clone(),
                first,
            }))
        })
    }
}

async fn shown(
    ctx: &Ctx,
    conn: &mut Conn,
    p: &Principal,
    batch: Id,
    rows: &[jobs::JobRow],
) -> ApiResult<EnvironmentBatch> {
    let mut members = Vec::new();
    for row in rows {
        let environment = match row.environment_id {
            Some(id) => Some(id),
            None => match row.node_id {
                Some(thread) => orch_db::queries::threads::thread(conn, thread)
                    .await?
                    .and_then(|row| row.environment_id),
                None => None,
            },
        };
        if let Some(environment) = environment {
            members.push(EnvironmentBatchJob {
                environment: super::super::show(ctx, conn, p, environment).await?.base,
                job: super::super::jobs::view(conn, row).await?,
            });
        }
    }
    Ok(EnvironmentBatch {
        batch_id: batch,
        jobs: members,
    })
}

enum Readback {
    First(Box<Queued>),
    Replay { batch: Id, jobs: Vec<AcceptedJob> },
}

impl Readback {
    fn batch(&self) -> Id {
        match self {
            Self::First(value) => value.batch,
            Self::Replay { batch, .. } => *batch,
        }
    }
    fn jobs(&self) -> Vec<AcceptedJob> {
        match self {
            Self::First(value) => value.jobs.iter().map(AcceptedJob::from).collect(),
            Self::Replay { jobs, .. } => jobs.clone(),
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
    async fn content(
        &self,
        conn: &mut Conn,
        environments: &[Id],
        extra: Vec<(ObjectKind, Id)>,
    ) -> ApiResult<native::Facts<Source>> {
        match self {
            Self::First(value) => common::content(conn, &value.origin, extra).await,
            Self::Replay { .. } => common::content_for(conn, environments, extra).await,
        }
    }
}

struct Content;

impl reply::Projection for Content {
    type Source = Source;
    type Input = Readback;
    type Subject = Vec<jobs::JobRow>;
    type Value = EnvironmentBatch;
    type Reply = EnvironmentBatch;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Readback) -> &'static Declaration<Source> {
        &RECOVER_CREATE.declaration
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let account = ctx.user()?.id();
            let mut rows = Vec::new();
            let mut content = Vec::new();
            let mut environments = std::collections::BTreeSet::new();
            for original in input.jobs() {
                let row = jobs::job(conn, original.id)
                    .await?
                    .filter(|row| {
                        row.batch_id == Some(input.batch())
                            && row.kind == original.kind
                            && row.environment_id == original.environment
                            && row.node_id == original.node
                            && row.created_by == Some(account)
                    })
                    .ok_or_else(ApiError::not_found)?;
                let environment = match row.environment_id {
                    Some(id) => Some(id),
                    None => match row.node_id {
                        Some(thread) => orch_db::queries::threads::thread(conn, thread)
                            .await?
                            .and_then(|row| row.environment_id),
                        None => None,
                    },
                };
                if let Some(environment) = environment {
                    environments.insert(environment);
                    content.push((ObjectKind::Environment, environment));
                }
                if let Some(machine) = super::super::jobs::waiting_machine(&row) {
                    content.push((ObjectKind::Machine, machine));
                }
                rows.push(row);
            }
            let mut facts = input.required(conn).await?;
            facts.push(
                input
                    .content(conn, &environments.into_iter().collect::<Vec<_>>(), content)
                    .await?,
            );
            Ok((rows, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        _: &'a Readback,
        rows: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for row in rows {
                if let Some(environment) = row.environment_id {
                    super::super::reading::decide(conn, p, environment).await?;
                }
                let job = super::super::jobs::prepare_read_for(conn, p, row.id).await?;
                if job.progress_only() {
                    return Err(ApiError::not_found());
                }
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Readback,
        rows: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move { Ok((shown(ctx, conn, p, input.batch(), rows).await?, Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Readback,
        opened: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move { Ok(opened.value.clone()) })
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
            Ok(old.subject.iter().all(|row| {
                fresh.subject.iter().any(|current| {
                    row.id == current.id
                        && row.kind == current.kind
                        && row.environment_id == current.environment_id
                        && row.node_id == current.node_id
                        && row.created_by == current.created_by
                })
            }))
        })
    }
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _: ForkCreateBody,
) -> ApiResult<api::EnvironmentRecoverCreateResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply_owned::<Confirm>(&ctx, tx).await?;
    ctx.app.engine.kick();
    let receipt = api::EnvironmentRecoverCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        batch_id: done.output.batch,
        job_ids: done.output.jobs.iter().map(|row| row.id).collect(),
    };
    publication::receipt(&done.ctx, &receipt)?;
    let mut input = done.output;
    if let Some(first) = input.first.take() {
        return Ok(api::EnvironmentRecoverCreateResponse::full(first));
    }
    Ok(
        match reply::render::<Content>(&done.ctx, &done.accepted, Readback::First(Box::new(input)))
            .await?
        {
            Some(batch) => api::EnvironmentRecoverCreateResponse::full(batch),
            None => api::EnvironmentRecoverCreateResponse::Receipt(receipt),
        },
    )
}
