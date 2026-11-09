//! One native batch selection and supplier pass through the shared effect and publication factories.

use super::preparation::engine_error;

use super::preparation::error;

use super::preparation::source_error;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_api_contract::fork_sources as rows;

use orch_api_contract::fork_sources::PlanSource as Role;

use orch_api_contract::forks::ForkBatchPlanReceipt;

use orch_api_contract::forks::ForkBatchPlanResponse;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_engine::fork_plan::operation as source;

use orch_engine::fork_plan::operation::BatchPreview as Intent;

use orch_engine::jobs::batch::PreparedBatch;

use orch_types::Time;

use std::collections::BTreeMap;

use std::collections::BTreeSet;

use std::sync::Arc;
const ROW: &Declaration<Role> = &rows::BATCH_PREFLIGHT;

pub(super) struct Preview;

pub(super) struct Input {
    prepared: PreparedBatch,
    inputs: Vec<source::Input>,
}

pub(super) struct Fetched {
    prepared: PreparedBatch,
    draft: BatchDraft,
    inputs: Vec<source::Input>,
    origins: Vec<source::Origin>,
    until: Option<Time>,
}

pub(super) struct Value {
    fetched: Arc<Fetched>,
    draft: BatchDraft,
}

pub(super) struct Accepted {
    plan: ForkBatchPlan,
    body: ForkBatchPreflightBody,
    roots: Vec<ObjectRef>,
    disclosure: Vec<ObjectRef>,
    actor: Id,
}

pub(super) fn collection(
    role: Role,
    targets: impl IntoIterator<Item = KeyResolvedTarget>,
) -> native::Facts<Role> {
    let mut groups = BTreeMap::<_, BTreeMap<_, _>>::new();
    for target in targets {
        groups
            .entry(target.object.kind)
            .or_default()
            .insert(target.object, target);
    }
    native::Facts::collection(
        role,
        groups
            .into_iter()
            .map(|(kind, targets)| (kind, targets.into_values().collect()))
            .collect(),
    )
}

fn roots(prepared: &PreparedBatch) -> Vec<ObjectRef> {
    if let Some(id) = prepared.body().all_of_environment {
        vec![ObjectRef {
            kind: ObjectKind::Environment,
            id,
        }]
    } else {
        prepared
            .selections()
            .iter()
            .map(|member| ObjectRef {
                kind: ObjectKind::Thread,
                id: member.member().thread,
            })
            .collect()
    }
}

async fn inputs(conn: &mut Conn, prepared: &PreparedBatch) -> ApiResult<Vec<source::Input>> {
    let mut result = Vec::new();
    for member in prepared.selections() {
        result.push(
            source::Input::selected(conn, member.selected())
                .await
                .map_err(engine_error)?,
        );
    }
    Ok(result)
}

async fn facts(
    conn: &mut Conn,
    prepared: &PreparedBatch,
    inputs: &[source::Input],
) -> ApiResult<Vec<native::Facts<Role>>> {
    if inputs.len() != prepared.selections().len() {
        return Err(ApiError::internal("the native batch input count changed"));
    }
    let mut selected = Vec::new();
    let mut copied = Vec::new();
    for input in inputs {
        for fact in source::facts::<Intent>(conn, input)
            .await
            .map_err(source_error)?
        {
            let targets = fact.groups.into_iter().flat_map(|(_, targets)| targets);
            match fact.source {
                Role::Sources => selected.extend(targets),
                Role::CopiedPages => copied.extend(targets),
                _ => {
                    return Err(ApiError::internal(
                        "the native batch supplied another source role",
                    ));
                }
            }
        }
    }
    // An empty native environment aggregate still has its actual governing environment.
    if let Some(id) = prepared.body().all_of_environment {
        selected.push(
            targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id,
                },
            )
            .await?,
        );
    }
    Ok(vec![
        collection(Role::Sources, selected),
        collection(Role::CopiedPages, copied),
    ])
}

impl Fetched {
    fn check(&self, ctx: &Ctx, caller: &Caller) -> ApiResult<()> {
        if self
            .until
            .is_some_and(|until| ctx.app.clock.time() >= until)
        {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the original batch request has ended",
            ));
        }
        for origin in &self.origins {
            origin
                .deadline(ctx.app.clock.as_ref(), caller)
                .map_err(engine_error)?;
        }
        Ok(())
    }
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: (),
    body: ForkBatchPreflightBody,
) -> crate::registry::OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = effects::prepare::<Preview>(ctx, conn, caller, query, body).await?;
        for origin in &effects::initial_value::<Preview>(ctx)?.origins {
            plan = plan
                .with_origin(ctx, conn, caller, origin.captured())
                .await?;
        }
        Ok(plan)
    })
}

impl effects::Initial for Preview {
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
        body: ForkBatchPreflightBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let prepared = match ctx
                .app
                .engine
                .prepare_batch_in_with_source::<Intent>(conn, caller, &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            let inputs = inputs(conn, &prepared).await?;
            let facts = facts(conn, &prepared, &inputs).await?;
            Ok((Input { prepared, inputs }, facts))
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
            let mut sources = Vec::new();
            for selected in &input.inputs {
                sources.push(source::Preparation::new(
                    source::Origin::capture::<Intent>(
                        caller,
                        selected.clone(),
                        credential.clone(),
                        until,
                    )
                    .map_err(engine_error)?,
                ));
            }
            let mut conn = ctx.app.db.acquire().await?;
            let draft = match ctx
                .app
                .engine
                .assemble_prepared_batch(&mut conn, caller, &input.prepared, Some(&sources))
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, &mut conn, why).await),
            };
            let mut origins = Vec::new();
            for source in &sources {
                origins.push(source.retained().await);
            }
            Ok(Fetched {
                prepared: input.prepared,
                draft,
                inputs: input.inputs,
                origins,
                until,
            })
        })
    }
}

impl effects::Effect for Preview {
    type Source = Role;
    type Query = ();
    type Body = ForkBatchPreflightBody;
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
        body: ForkBatchPreflightBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let fetched = effects::initial_value::<Preview>(ctx)?;
            if &body != fetched.prepared.body() {
                return Err(ApiError::internal("the native batch has another request"));
            }
            source::lock_all(conn, &fetched.inputs)
                .await
                .map_err(source_error)?;
            if let Some(id) = body.all_of_environment {
                sqlx::query("SELECT id FROM environments WHERE id=$1 FOR KEY SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            let facts = facts(conn, &fetched.prepared, &fetched.inputs).await?;
            let mut plan = native::bind_required(ctx, ROW, &facts)?;
            for origin in &fetched.origins {
                plan = plan
                    .with_origin(ctx, conn, caller, origin.captured())
                    .await?;
            }
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            fetched.check(ctx, principal.caller())?;
            let current = match ctx
                .app
                .engine
                .prepare_batch_in_with_source::<Intent>(conn, principal.caller(), &body)
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            if !fetched.prepared.preserves(&current) {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the native batch selection changed",
                ));
            }
            let mut draft = fetched.draft.clone();
            complete_batch(conn, &principal, &mut draft).await?;
            Ok((Value { fetched, draft }, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { value.fetched.check(ctx, principal.caller()) })
    }
}

impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Accepted>>> {
        Box::pin(async move {
            let plan = match ctx
                .app
                .engine
                .keep_batch_plan_on(conn, principal.caller(), value.draft.clone(), || {
                    value
                        .fetched
                        .check(ctx, principal.caller())
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))?;
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                })
                .await
            {
                Ok(value) => value,
                Err(why) => return Err(error(ctx, conn, why).await),
            };
            Ok(effects::Outcome::Applied(Accepted {
                plan,
                body: value.draft.body.clone(),
                roots: roots(&value.fetched.prepared),
                disclosure: value.draft.disclosure.clone(),
                actor: principal.user(),
            }))
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _body: ForkBatchPreflightBody,
) -> ApiResult<ForkBatchPlanResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    guard.check_deadline(&done.ctx)?;
    effects::initial_value::<Preview>(&done.ctx)?.check(&done.ctx, &done.accepted)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let planned = done.output.plan.plans.len();
    let executable = usize::try_from(done.output.plan.executable).map_err(ApiError::internal)?;
    let receipt = ForkBatchPlanReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: done.output.plan.preflight_id,
        planned,
        executable,
        blocked: planned.saturating_sub(executable),
        skipped: done.output.plan.skipped.len(),
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Readback>(&done.ctx, &done.accepted, done.output).await? {
        Some(plan) => Ok(ForkBatchPlanResponse::Full(Box::new(plan))),
        None => Ok(ForkBatchPlanResponse::Receipt(receipt)),
    }
}

struct Readback;

impl reply::Projection for Readback {
    type Source = Role;
    type Input = Accepted;
    type Subject = orch_db::queries::jobs::forks::PreflightRow;
    type Value = Vec<ObjectRef>;
    type Reply = ForkBatchPlan;
    const READ_SOURCE: Role = Role::Subjects;
    fn declaration(_input: &Accepted) -> &'static Declaration<Role> {
        ROW
    }
    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Accepted,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let row = orch_db::queries::jobs::forks::preflight(conn, input.plan.preflight_id)
                .await?
                .filter(|row| {
                    row.kind == orch_db::queries::jobs::forks::BATCH_PLAN_KIND
                        && row.created_by == input.actor
                        && row.environment_id == input.body.all_of_environment
                })
                .ok_or_else(ApiError::not_found)?;
            let stored: orch_engine::fork_plan::StoredBatch =
                serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
            if stored.plan != input.plan || stored.body != input.body {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the retained native batch changed",
                ));
            }
            let mut targets = Vec::new();
            for object in &input.roots {
                targets.push(targets::page_object(conn, *object).await?);
            }
            Ok((
                row,
                vec![
                    collection(Role::Sources, targets.clone()),
                    collection(Role::CopiedPages, vec![]),
                    collection(Role::Subjects, targets),
                ],
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Accepted,
        _subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            // Hidden environment members stay redacted. Previously named members need their current native visibility.
            for source in input
                .plan
                .plans
                .iter()
                .map(|plan| &plan.source)
                .chain(input.plan.skipped.iter().map(|skipped| &skipped.thread))
            {
                if !source.redacted {
                    authorization::require(
                        conn,
                        principal,
                        source.id,
                        authorization::VIEW,
                        Act::Use,
                        Policy::default(),
                    )
                    .await?;
                }
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Accepted,
        _subject: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let mut objects = input
                .disclosure
                .iter()
                .map(|object| (object.kind, object.id))
                .collect::<BTreeSet<_>>();
            for skipped in &input.plan.skipped {
                if thread_rows::thread(conn, skipped.thread.id)
                    .await?
                    .filter(|thread| thread.deleted_at.is_none())
                    .is_some()
                {
                    objects.insert((ObjectKind::Thread, skipped.thread.id));
                }
            }
            let objects = objects
                .into_iter()
                .map(|(kind, id)| ObjectRef { kind, id })
                .collect::<Vec<_>>();
            let mut targets = Vec::new();
            for object in &objects {
                targets.push(targets::page_object(conn, *object).await?);
            }
            Ok((objects, vec![collection(Role::Content, targets)]))
        })
    }
    fn render<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a Accepted,
        _prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ForkBatchPlan>> {
        Box::pin(async move { Ok(input.plan.clone()) })
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
            Ok(old.subject.id == fresh.subject.id
                && old.subject.plan == fresh.subject.plan
                && old.value == fresh.value)
        })
    }
}
