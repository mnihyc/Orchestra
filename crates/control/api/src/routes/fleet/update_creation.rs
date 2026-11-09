//! Confirmation consumes the reviewed native members and keeps both original operations.
use super::update_operation::{self, source_error as ended};
use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, Apply, Effect, FirstEffect, Initial, Outcome},
        reply,
    },
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    common::{ErrorCode, ReceiptProjection},
    environment_sources::update_batch::{self as declarations, Source},
    forks::UpdateConfirmQuery,
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_db::queries::jobs::{self, forks::PreflightRow};
use orch_engine::{jobs::recreate::environment_authority, updates::operation::Input as Origin};
use std::collections::BTreeMap;

const CREATION_KIND: &str = "environment_update_batch";

#[derive(serde::Serialize, serde::Deserialize)]
struct Accepted {
    jobs: Vec<AcceptedJob>,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct AcceptedJob {
    environment_id: Id,
    job_id: Id,
}
impl Accepted {
    fn from_rows(rows: &[jobs::JobRow]) -> ApiResult<Self> {
        Ok(Self {
            jobs: rows
                .iter()
                .map(|row| {
                    Ok(AcceptedJob {
                        environment_id: row.environment_id.ok_or_else(ApiError::not_found)?,
                        job_id: row.id,
                    })
                })
                .collect::<ApiResult<_>>()?,
        })
    }
    fn receipt(&self, batch: Id) -> api::EnvironmentUpdateBatchCreateResponse {
        api::EnvironmentUpdateBatchCreateResponse::Receipt(
            api::EnvironmentUpdateBatchCreateReceipt {
                projection: ReceiptProjection::Receipt,
                batch_id: batch,
                jobs: self
                    .jobs
                    .iter()
                    .map(|row| api::EnvironmentUpdateBatchJobReceipt {
                        environment_id: row.environment_id,
                        job_id: row.job_id,
                    })
                    .collect(),
            },
        )
    }
}
pub(super) fn binding() -> crate::middleware::idempotency::Creation<
    UpdateConfirmQuery,
    ForkCreateBody,
    api::EnvironmentUpdateBatchCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: CREATION_KIND,
        replay: |ctx, _, _, created| {
            Box::pin(async move {
                if created.kind != CREATION_KIND {
                    return Err(ApiError::internal("the accepted native batch kind changed"));
                }
                let accepted: Accepted =
                    serde_json::from_value(created.data).map_err(ApiError::internal)?;
                let fallback = accepted.receipt(created.id);
                crate::key_operations::publication::receipt(&ctx, &fallback)?;
                match reply::render_completed_replay::<Projection>(
                    &ctx,
                    ctx.user()?.id(),
                    Readback::Replay {
                        batch: created.id,
                        accepted,
                    },
                )
                .await
                {
                    Ok(Some(full)) => Ok(api::EnvironmentUpdateBatchCreateResponse::Full(full)),
                    Ok(None) => Ok(fallback),
                    Err(error)
                        if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) =>
                    {
                        Ok(fallback)
                    }
                    Err(error) => Err(error),
                }
            })
        },
        identity: |response| {
            let (batch, accepted) = match response {
                api::EnvironmentUpdateBatchCreateResponse::Full(value) => (
                    value.batch_id,
                    Accepted {
                        jobs: value
                            .jobs
                            .iter()
                            .map(|row| AcceptedJob {
                                environment_id: row.environment.id,
                                job_id: row.job.id,
                            })
                            .collect(),
                    },
                ),
                api::EnvironmentUpdateBatchCreateResponse::Receipt(value) => (
                    value.batch_id,
                    Accepted {
                        jobs: value
                            .jobs
                            .iter()
                            .map(|row| AcceptedJob {
                                environment_id: row.environment_id,
                                job_id: row.job_id,
                            })
                            .collect(),
                    },
                ),
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION_KIND.into(),
                id: batch,
                data: serde_json::to_value(accepted).map_err(ApiError::internal)?,
            })
        },
    }
}

enum Readback {
    First(Box<Saved>),
    Replay { batch: Id, accepted: Accepted },
}
impl Readback {
    fn batch(&self) -> Id {
        match self {
            Self::First(saved) => saved.batch,
            Self::Replay { batch, .. } => *batch,
        }
    }
}

pub(super) struct Create;
pub(super) struct Intent {
    ids: Vec<Id>,
    options: Option<orch_api_contract::forks::ChangeOptions>,
    review: Id,
    expected: BTreeMap<Id, Id>,
}
struct Member {
    draft: updates::Draft,
    review: Option<Origin>,
    confirmation: Origin,
    lives: Vec<(Caller, Option<Time>)>,
}
pub(super) struct Prepared {
    preflight: Id,
    batch: Id,
    members: Vec<Member>,
    notify: bool,
    accepted: Caller,
    until: Option<Time>,
    facts: Vec<native::Facts<Source>>,
}
pub(super) struct Saved {
    batch: Id,
    jobs: Vec<jobs::JobRow>,
    facts: Vec<native::Facts<Source>>,
    origins: Vec<Origin>,
    accepted: Caller,
    until: Option<Time>,
}
fn expired() -> ApiError {
    plans::refused(UpdateError::Expired)
}
async fn locate(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    id: Id,
    lock: bool,
) -> ApiResult<PreflightRow> {
    let row = if lock {
        jobs::changes::lock_plan(conn, id).await?
    } else {
        jobs::forks::preflight(conn, id).await?
    }
    .ok_or_else(ApiError::not_found)?;
    if row.kind != "update_batch"
        || row.node_id.is_some()
        || row.environment_id.is_some()
        || row.created_by != caller.user()
        || row.plan["providers"] != false
    {
        return Err(ApiError::not_found());
    }
    if row.expires_at <= ctx.app.clock.now() {
        return Err(expired());
    }
    Ok(row)
}
fn recipes(row: &PreflightRow) -> ApiResult<Vec<updates::Recipe>> {
    let mut recipes: Vec<updates::Recipe> =
        serde_json::from_value(row.plan["recipes"].clone()).map_err(ApiError::internal)?;
    recipes.sort_by_key(|recipe| recipe.target.id());
    if recipes
        .iter()
        .any(|recipe| !matches!(recipe.target, Target::Environment(_)))
        || recipes
            .windows(2)
            .any(|members| members[0].target == members[1].target)
    {
        return Err(ApiError::not_found());
    }
    Ok(recipes)
}
fn reviews(row: &PreflightRow, caller: &Caller) -> ApiResult<BTreeMap<Id, Origin>> {
    match row.plan.get("update_review_sources") {
        Some(value) => {
            let origins: BTreeMap<Id, Origin> =
                serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
            let mut selected = BTreeMap::new();
            for (id, origin) in origins {
                if origin.origin().operation() != declarations::GLOBAL_PREFLIGHT.operation
                    || origin.origin().user() != caller.user()
                    || id != origin.environment()
                    || origin.node_scope().is_some()
                    || selected.insert(id, origin).is_some()
                {
                    return Err(ApiError::not_found());
                }
            }
            Ok(selected)
        }
        None if native::modern(caller) => Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the original native review is not recorded",
        )),
        None => Ok(BTreeMap::new()),
    }
}
async fn known(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    recipes: &[updates::Recipe],
) -> ApiResult<crate::registry::OperationPlan> {
    let ids = recipes
        .iter()
        .map(|recipe| recipe.target.id())
        .collect::<Vec<_>>();
    let mut templates = std::collections::BTreeSet::new();
    for recipe in recipes {
        if let Some((selected, old)) =
            updates::environment_update_template(conn, recipe.target.id())
                .await
                .map_err(ended)?
            && selected != old
        {
            templates.insert(selected);
        }
    }
    let facts = update_operation::confirmation::required_facts(
        conn,
        &ids,
        &templates.into_iter().collect::<Vec<_>>(),
    )
    .await?;
    let plan = native::bind_known(ctx, &declarations::GLOBAL_CREATE, &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    for recipe in recipes {
        if !updates::may_manage(conn, &principal, recipe.target)
            .await
            .map_err(ApiError::internal)?
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "manage permission on the update environment is required",
            ));
        }
    }
    Ok(plan)
}
impl Effect for Create {
    type Source = Source;
    type Query = UpdateConfirmQuery;
    type Body = ForkCreateBody;
    type Value = Prepared;
    type Output = Saved;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &declarations::GLOBAL_CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: UpdateConfirmQuery,
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let observed = effects::initial_value::<Create>(ctx)?;
            let bounded = caller
                .intersect_restrictions(&observed.accepted)
                .map_err(ApiError::internal)?;
            let caller = &bounded;
            update_operation::retained_deadline(ctx, caller, observed.until)?;
            let row = locate(ctx, conn, caller, body.preflight_id, true).await?;
            let recipes = recipes(&row)?;
            let mut reviews = reviews(&row, caller)?;
            let known = known(ctx, conn, caller, &recipes).await?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let mut members = Vec::new();
            for old in recipes {
                let id = old.target.id();
                let observation = observed.observations.get(&id).ok_or_else(expired)?;
                let original = observed.sources.get(&id).ok_or_else(expired)?.clone();
                original.lock(conn).await.map_err(ended)?;
                let draft = ctx
                    .app
                    .engine
                    .replay_update(
                        conn,
                        &principal,
                        old.target,
                        api::UpdatePreflightReason::Update,
                        body.options.as_ref(),
                        observation,
                    )
                    .await
                    .map_err(update_operation::update_error)?;
                if draft.recipe.old != old.old || draft.recipe.hash != old.hash {
                    return Err(ApiError::new(
                        ErrorCode::ConflictStaleRevision,
                        "a unit or its selected target changed after this batch was planned; review a new plan",
                    ));
                }
                if !draft.blockers.is_empty() {
                    let error = plans::refused(UpdateError::Blocked(draft.blockers.clone()));
                    return Err(if native::modern(caller) {
                        authorization::problem_without_content(error)
                    } else {
                        error
                    });
                }
                let current = original
                    .current_authority(conn, ctx.app.clock.as_ref())
                    .await
                    .map_err(ended)?;
                let confirmation = current.input;
                let mut lives = vec![(current.caller, current.until)];
                let review = match reviews.remove(&id) {
                    Some(review) => {
                        let current = review
                            .current_authority(conn, ctx.app.clock.as_ref())
                            .await
                            .map_err(ended)?;
                        if !review.same_source(&confirmation) {
                            return Err(expired());
                        }
                        lives.push((current.caller, current.until));
                        Some(current.input)
                    }
                    None if row.plan.get("update_review_sources").is_some() => {
                        return Err(ApiError::not_found());
                    }
                    None => None,
                };
                members.push(Member {
                    draft,
                    review,
                    confirmation,
                    lives,
                });
            }
            let ids = members
                .iter()
                .map(|member| member.draft.recipe.target.id())
                .collect::<Vec<_>>();
            let mut templates = std::collections::BTreeSet::new();
            for member in &members {
                if let Some((selected, old)) =
                    updates::environment_update_template(conn, member.draft.recipe.target.id())
                        .await
                        .map_err(ended)?
                    && selected != old
                {
                    templates.insert(selected);
                }
            }
            let mut facts = update_operation::confirmation::required_facts(
                conn,
                &ids,
                &templates.into_iter().collect::<Vec<_>>(),
            )
            .await?;
            facts.push(update_operation::environment_facts(conn, Source::Content, &ids).await?);
            let value = Prepared {
                preflight: row.id,
                batch: new_id(),
                members,
                notify: query.notify.unwrap_or(true),
                accepted: caller.clone(),
                until: observed.until,
                facts: facts.clone(),
            };
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for member in &value.members {
                if !updates::may_manage(conn, principal, member.draft.recipe.target)
                    .await
                    .map_err(ApiError::internal)?
                {
                    return Err(ApiError::not_found());
                }
            }
            Ok(())
        })
    }
}
impl Initial for Create {
    type Input = Intent;
    type Fetched = update_operation::Fetched;
    fn declaration(_: &Intent) -> &'static Declaration<Source> {
        &declarations::GLOBAL_CREATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: UpdateConfirmQuery,
        body: ForkCreateBody,
    ) -> BoxFuture<'a, ApiResult<(Intent, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = locate(ctx, conn, caller, body.preflight_id, false).await?;
            let recipes = recipes(&row)?;
            let _ = known(ctx, conn, caller, &recipes).await?;
            let mut reviews = reviews(&row, caller)?;
            let mut values = Vec::new();
            let mut ids = Vec::new();
            for recipe in &recipes {
                let id = recipe.target.id();
                match reviews.remove(&id) {
                    Some(review) => {
                        let _ = review
                            .current_authority(conn, ctx.app.clock.as_ref())
                            .await
                            .map_err(ended)?;
                    }
                    None if row.plan.get("update_review_sources").is_some() => {
                        return Err(ApiError::not_found());
                    }
                    None => {}
                }
                ids.push(id);
                values.push(
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id,
                        },
                    )
                    .await?,
                );
            }
            Ok((
                Intent {
                    ids,
                    options: body.options,
                    review: body.preflight_id,
                    expected: recipes
                        .iter()
                        .map(|recipe| (recipe.target.id(), recipe.old))
                        .collect(),
                },
                vec![native::Facts::collection(
                    Source::Environments,
                    if values.is_empty() {
                        Vec::new()
                    } else {
                        vec![(KeyTargetKind::Object(ObjectKind::Environment), values)]
                    },
                )],
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Intent,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for id in &input.ids {
                if !updates::may_manage(conn, principal, Target::Environment(*id))
                    .await
                    .map_err(ApiError::internal)?
                {
                    return Err(ApiError::not_found());
                }
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Intent,
    ) -> BoxFuture<'a, ApiResult<Self::Fetched>> {
        Box::pin(async move {
            let selection = update_operation::Selection {
                node: None,
                visible: input.ids.clone(),
                managed: input.ids,
            };
            update_operation::observe(
                ctx,
                caller,
                &selection,
                input.options.as_ref(),
                Some(&input.expected),
                Some(input.review),
            )
            .await
        })
    }
}
fn live(member: &Member, clock: &dyn orch_types::Clock) -> ApiResult<()> {
    if member.lives.iter().any(|(caller, until)| {
        !caller.key_deadline_allows(clock) || until.is_some_and(|until| clock.time() >= until)
    }) {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the original update authority has ended",
        ));
    }
    Ok(())
}
impl Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Saved>>> {
        Box::pin(async move {
            update_operation::retained_deadline(ctx, &value.accepted, value.until)?;
            let deadline = before.deadline();
            let mut first = Some(before);
            let mut queued = Vec::new();
            let mut affected = Vec::new();
            for member in &value.members {
                let row = orch_engine::jobs::recreate::queue_before(
                    conn,
                    member.draft.clone(),
                    Some(value.batch),
                    ctx.now,
                    || {
                        update_operation::retained_deadline(ctx, &value.accepted, value.until)
                            .map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })?;
                        for member in &value.members {
                            live(member, ctx.app.clock.as_ref()).map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })?;
                        }
                        match first.take() {
                            Some(first) => first.check(),
                            None => deadline.check(),
                        }
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                    },
                )
                .await
                .map_err(|error| match error {
                    UpdateError::Internal(orch_engine::EngineError::BeforeEffect(error)) => {
                        match error.downcast::<ApiError>() {
                            Ok(error) => *error,
                            Err(error) => ApiError::internal(error),
                        }
                    }
                    error => plans::refused(error),
                })?;
                environment_authority::retain(
                    conn,
                    &row,
                    value.batch,
                    member.review.clone(),
                    member.confirmation.clone(),
                    ctx.app.clock.time(),
                )
                .await
                .map_err(ApiError::engine)?;
                affected.extend(member.draft.recipe.threads.iter().copied());
                queued.push(row);
            }
            if let Some(first) = first.take() {
                first.check()?;
            }
            if value.notify {
                plans::tell_update(ctx, conn, &affected, value.batch).await?;
            }
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(ctx.user()?.id()),
                CREATION_KIND,
                value.batch,
                serde_json::to_value(Accepted::from_rows(&queued)?).map_err(ApiError::internal)?,
            )
            .await?;
            jobs::forks::forget_preflight(conn, value.preflight).await?;
            deadline.check()?;
            update_operation::retained_deadline(ctx, &value.accepted, value.until)?;
            for member in &value.members {
                live(member, ctx.app.clock.as_ref())?;
            }
            Ok(Outcome::Applied(Saved {
                batch: value.batch,
                jobs: queued,
                facts: value.facts.clone(),
                accepted: value.accepted.clone(),
                until: value.until,
                origins: value
                    .members
                    .iter()
                    .flat_map(|member| {
                        member
                            .review
                            .iter()
                            .cloned()
                            .chain(std::iter::once(member.confirmation.clone()))
                    })
                    .collect(),
            }))
        })
    }
}
struct Projection;
impl reply::Projection for Projection {
    type Source = Source;
    type Input = Readback;
    type Subject = Vec<jobs::JobRow>;
    type Value = update_operation::readback::Value;
    type Reply = api::EnvironmentBatch;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Readback) -> &'static Declaration<Source> {
        &declarations::GLOBAL_CREATE
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Readback,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (rows, mut facts) = match input {
                Readback::First(saved) => (saved.jobs.clone(), saved.facts.clone()),
                Readback::Replay { batch, accepted } => {
                    let account = ctx.user()?.id();
                    let mut rows = Vec::new();
                    for identity in &accepted.jobs {
                        let row = jobs::job(conn, identity.job_id)
                            .await?
                            .ok_or_else(ApiError::not_found)?;
                        if row.kind != "environment_recreate"
                            || row.environment_id != Some(identity.environment_id)
                            || row.provider_id.is_some()
                            || row.batch_id != Some(*batch)
                            || row.created_by != Some(account)
                        {
                            return Err(ApiError::not_found());
                        }
                        rows.push(row);
                    }
                    let ids = accepted
                        .jobs
                        .iter()
                        .map(|row| row.environment_id)
                        .collect::<Vec<_>>();
                    let content =
                        update_operation::environment_facts(conn, Source::Content, &ids).await?;
                    (rows, vec![content])
                }
            };
            for fact in &mut facts {
                for (_, targets) in &mut fact.groups {
                    for target in targets {
                        let (KeyTargetKind::Object(kind), Some(id)) =
                            (target.object.kind, target.object.id)
                        else {
                            return Err(ApiError::not_found());
                        };
                        *target = targets::page_object(conn, ObjectRef { kind, id }).await?;
                    }
                }
            }
            Ok((rows, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Readback,
        rows: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for row in rows {
                let id = row.environment_id.ok_or_else(ApiError::not_found)?;
                environments::reading::decide(conn, principal, id).await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Readback,
        rows: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let ids = rows
                .iter()
                .filter_map(|row| row.environment_id)
                .collect::<Vec<_>>();
            update_operation::readback::prepare_context(
                ctx,
                conn,
                principal,
                &declarations::GLOBAL_CREATE,
                &ids,
                &ids,
                &[],
            )
            .await
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Readback,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let references =
                update_operation::readback::references(ctx, conn, principal, &prepared.value)
                    .await?;
            let mut jobs = Vec::new();
            for row in &prepared.subject {
                let id = row.environment_id.ok_or_else(ApiError::not_found)?;
                jobs.push(api::EnvironmentBatchJob {
                    environment: references.get(&id).ok_or_else(ApiError::not_found)?.clone(),
                    job: shown_jobs::view(conn, row).await?,
                });
            }
            Ok(api::EnvironmentBatch {
                batch_id: input.batch(),
                jobs,
            })
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
            Ok(old.subject.len() == fresh.subject.len()
                && old.subject.iter().zip(&fresh.subject).all(|(old, fresh)| {
                    old.id == fresh.id
                        && old.kind == fresh.kind
                        && old.environment_id == fresh.environment_id
                        && old.batch_id == fresh.batch_id
                        && old.created_by == fresh.created_by
                })
                && update_operation::readback::preserves(&old.value, &fresh.value))
        })
    }
}
pub(super) async fn create(
    ctx: Ctx,
    _: UpdateConfirmQuery,
    _: ForkCreateBody,
) -> ApiResult<api::EnvironmentUpdateBatchCreateResponse> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<Create>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&ctx.app)?;
    if !completed
        .accepted
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the update confirmation has ended",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    let fallback = api::EnvironmentUpdateBatchCreateResponse::Receipt(
        api::EnvironmentUpdateBatchCreateReceipt {
            projection: ReceiptProjection::Receipt,
            batch_id: completed.output.batch,
            jobs: completed
                .output
                .jobs
                .iter()
                .map(|row| {
                    Ok(api::EnvironmentUpdateBatchJobReceipt {
                        environment_id: row.environment_id.ok_or_else(ApiError::not_found)?,
                        job_id: row.id,
                    })
                })
                .collect::<ApiResult<_>>()?,
        },
    );
    crate::key_operations::publication::receipt(&completed.ctx, &fallback)?;
    let reader = completed
        .accepted
        .intersect_restrictions(&completed.output.accepted)
        .map_err(ApiError::internal)?;
    completed
        .ctx
        .publication
        .retain_deadline(completed.output.until)?;
    let Some(accepted) =
        update_operation::response_caller(&completed.ctx, &reader, &completed.output.origins)
            .await?
    else {
        return Ok(fallback);
    };
    Ok(
        match reply::render::<Projection>(
            &completed.ctx,
            &accepted,
            Readback::First(Box::new(completed.output)),
        )
        .await?
        {
            Some(full) => api::EnvironmentUpdateBatchCreateResponse::Full(full),
            None => fallback,
        },
    )
}
