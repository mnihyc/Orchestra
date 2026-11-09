//! One native confirmation observation, repeatable locked queue preparation, and the same writer.

use super::*;
use orch_api_contract::move_batch_sources::CREATE;
use orch_db::queries::environments::hosting as envs;
use orch_engine::jobs::environment_move;

const CREATION_KIND: &str = "node_environment_move_batch";
#[derive(serde::Serialize, serde::Deserialize)]
struct Accepted {
    node: Id,
    jobs: Vec<(Id, Id)>,
}
enum Readback {
    First(Started),
    Replay { batch: Id, accepted: Accepted },
}
impl Readback {
    fn batch(&self) -> Id {
        match self {
            Self::First(value) => value.batch,
            Self::Replay { batch, .. } => *batch,
        }
    }
    fn node(&self) -> Id {
        match self {
            Self::First(value) => value.node,
            Self::Replay { accepted, .. } => accepted.node,
        }
    }
    fn jobs(&self) -> &[(Id, Id)] {
        match self {
            Self::First(value) => &value.jobs,
            Self::Replay { accepted, .. } => &accepted.jobs,
        }
    }
    async fn required(&self, conn: &mut Conn) -> ApiResult<Vec<native::Facts<Source>>> {
        match self {
            Self::First(value) => {
                facts(
                    conn,
                    &value.jobs.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
                    &value.destinations,
                )
                .await
            }
            Self::Replay { .. } => Ok(Vec::new()),
        }
    }
}
pub(in crate::routes) fn binding() -> crate::middleware::idempotency::Creation<
    (),
    ForkCreateBody,
    api::NodeEnvironmentMoveCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: CREATION_KIND,
        identity: |value| {
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION_KIND.into(),
                id: match value {
                    api::NodeEnvironmentMoveCreateResponse::Full(value) => value.batch_id,
                    api::NodeEnvironmentMoveCreateResponse::Receipt(value) => value.batch_id,
                },
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                if created.kind != CREATION_KIND {
                    return Err(ApiError::internal(
                        "the accepted native move batch kind changed",
                    ));
                }
                let accepted: Accepted =
                    serde_json::from_value(created.data).map_err(ApiError::internal)?;
                if ctx.id("id")? != accepted.node {
                    return Err(ApiError::not_found());
                }
                let receipt = api::NodeEnvironmentMoveCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    node_id: accepted.node,
                    batch_id: created.id,
                    job_ids: accepted.jobs.iter().map(|(_, id)| *id).collect(),
                };
                publication::receipt(&ctx, &receipt)?;
                match reply::render_completed_replay::<readback::Content>(
                    &ctx,
                    ctx.user()?.id(),
                    Readback::Replay {
                        batch: created.id,
                        accepted,
                    },
                )
                .await
                {
                    Ok(Some(value)) => Ok(api::NodeEnvironmentMoveCreateResponse::Full(Box::new(
                        value,
                    ))),
                    Ok(None) => Ok(api::NodeEnvironmentMoveCreateResponse::Receipt(receipt)),
                    Err(error)
                        if matches!(
                            error.code(),
                            orch_api_contract::common::ErrorCode::NotFound
                                | orch_api_contract::common::ErrorCode::AuthzDenied
                        ) =>
                    {
                        Ok(api::NodeEnvironmentMoveCreateResponse::Receipt(receipt))
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(in crate::routes) struct Confirm;
pub(in crate::routes) struct Review {
    node: Id,
    body: ForkCreateBody,
    row: PreflightRow,
    stored: StoredMoveBatch,
}
pub(in crate::routes) struct Fetched {
    review: Review,
    hosted: Vec<MoveDraft>,
    portable: Vec<machine_move::Draft>,
    origin: environment_move::authority::Origin,
}
pub(in crate::routes) struct Ready {
    fetched: Arc<Fetched>,
    hosted: Vec<environment_move::PreparedQueue>,
    portable: Vec<machine_move::PreparedQueue>,
    recipients: Vec<orch_engine::notifications::OperationRecipient>,
    destinations: BTreeSet<(ObjectKind, Id)>,
}
pub(in crate::routes) struct Started {
    node: Id,
    batch: Id,
    jobs: Vec<(Id, Id)>,
    destinations: BTreeSet<(ObjectKind, Id)>,
}
fn members(stored: &StoredMoveBatch) -> Vec<Id> {
    stored
        .recipes
        .iter()
        .map(|recipe| recipe.replacement.target.id())
        .chain(
            stored
                .portable_recipes
                .iter()
                .map(|recipe| recipe.environment),
        )
        .collect()
}
fn destinations(stored: &StoredMoveBatch) -> BTreeSet<(ObjectKind, Id)> {
    let mut selected = BTreeSet::new();
    for recipe in &stored.recipes {
        selected.insert((ObjectKind::Host, recipe.destination));
        selected.insert((ObjectKind::Template, recipe.replacement.template));
    }
    for recipe in &stored.portable_recipes {
        selected.extend(
            recipe
                .destination_access()
                .into_iter()
                .map(|(object, _)| (object.kind, object.id)),
        );
    }
    selected
}
async fn facts(
    conn: &mut Conn,
    ids: &[Id],
    destinations: &BTreeSet<(ObjectKind, Id)>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![
        objects(
            conn,
            Source::Environments,
            ids.iter().map(|id| (ObjectKind::Environment, *id)),
        )
        .await?,
        objects(conn, Source::Destinations, destinations.iter().copied()).await?,
    ])
}
async fn current_review(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: ForkCreateBody,
    lock: bool,
) -> ApiResult<Review> {
    let node = ctx.id("id")?;
    let row = if lock {
        jobs::changes::lock_plan(conn, body.preflight_id).await?
    } else {
        jobs::forks::preflight(conn, body.preflight_id).await?
    }
    .ok_or_else(ApiError::not_found)?;
    if row.kind != "move_batch" || row.node_id != Some(node) || row.created_by != caller.user() {
        return Err(ApiError::not_found());
    }
    if Time::new(row.expires_at) <= ctx.app.clock.time() {
        return Err(refused(updates::UpdateError::Expired));
    }
    let stored: StoredMoveBatch =
        serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
    let under = envs::environments_under(conn, node).await?;
    if members(&stored).iter().any(|id| !under.contains(id)) {
        return Err(ApiError::not_found());
    }
    Ok(Review {
        node,
        body,
        row,
        stored,
    })
}
async fn require_members(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    review: &Review,
) -> ApiResult<()> {
    if Time::new(review.row.expires_at) <= ctx.app.clock.time() {
        return Err(refused(updates::UpdateError::Expired));
    }
    node(conn, principal, review.node).await?;
    for id in members(&review.stored) {
        if !updates::may_manage(conn, principal, updates::Target::Environment(id))
            .await
            .map_err(ApiError::engine)?
        {
            return Err(refused(updates::UpdateError::Denied));
        }
    }
    Ok(())
}
async fn require_places(
    conn: &mut Conn,
    principal: &Principal,
    selected: &BTreeSet<(ObjectKind, Id)>,
) -> ApiResult<()> {
    for (kind, id) in selected {
        let permission = match kind {
            ObjectKind::Host => Permission::HostUse,
            ObjectKind::Machine => Permission::MachineUse,
            ObjectKind::Template => Permission::TemplateUse,
            _ => return Err(ApiError::internal("unsupported native move destination")),
        };
        if !orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: *kind,
                id: *id,
            },
            permission,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .is_allowed()
        {
            return Err(refused(updates::UpdateError::Denied));
        }
    }
    Ok(())
}
impl effects::Initial for Confirm {
    type Input = Review;
    type Fetched = Fetched;
    fn declaration(_: &Review) -> &'static Declaration<Source> {
        &CREATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Review, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let review = current_review(ctx, conn, caller, body, false).await?;
            let selected = facts(
                conn,
                &members(&review.stored),
                &destinations(&review.stored),
            )
            .await?;
            Ok((review, selected))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        review: &'a Review,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_members(ctx, conn, principal, review).await?;
            require_places(conn, principal, &destinations(&review.stored)).await
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        review: Review,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let selected = facts(
                &mut tx,
                &members(&review.stored),
                &destinations(&review.stored),
            )
            .await?;
            let known = native::bind_known(ctx, &CREATE, &selected)?;
            let principal = native::preparation_principal(ctx, &mut tx, caller, &known).await?;
            let captured = orch_authz::CapturedOperation::capture(principal.caller())
                .map_err(ApiError::internal)?;
            let http = native::http_origin::capture_operation(ctx, &mut tx, &captured).await?;
            let planning = orch_engine::jobs::lifecycle_commands::planning::Planning::node(
                &mut tx,
                http,
                review.row.id,
                review.node,
                ctx.app.clock.as_ref(),
            )
            .await
            .map_err(ApiError::engine)?;
            let mut hosted = Vec::new();
            let mut portable = Vec::new();
            planning
                .scope(async {
                    for recipe in &review.stored.recipes {
                        let draft = ctx
                            .app
                            .engine
                            .draft_move(
                                &mut tx,
                                &principal,
                                recipe.replacement.target.id(),
                                &review.stored.request,
                                review.body.options.as_ref(),
                            )
                            .await
                            .map_err(refused)?;
                        if draft.recipe.replacement.old != recipe.replacement.old {
                            return Err(ApiError::new(
                                orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                                "a unit changed after this batch was planned",
                            ));
                        }
                        hosted.push(draft);
                    }
                    Ok::<(), ApiError>(())
                })
                .await?;
            tx.rollback().await.map_err(DbError::from)?;
            planning.scope(async {
            for recipe in &review.stored.portable_recipes {
                let draft = ctx
                    .app
                    .engine
                    .draft_machine_move(
                        &principal,
                        recipe.environment,
                        &review.stored.request,
                        review.body.options.as_ref(),
                    )
                    .await
                    .map_err(refused)?;
                if !recipe.same_choice(&draft.recipe) {
                    return Err(ApiError::new(
                        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                        "A source, destination or promised data changed; review the batch again.",
                    ));
                }
                portable.push(draft);
            }
            Ok::<(),ApiError>(())
            }).await?;
            let http = planning.retained();
            let mut tail = ctx.app.db.begin().await?;
            let _ = native::http_origin::current(ctx, &mut tail, &http).await?;
            tail.rollback().await.map_err(DbError::from)?;
            let origin = environment_move::authority::Origin::from_http(http, review.node)
                .map_err(ApiError::engine)?;
            Ok(Fetched {
                review,
                hosted,
                portable,
                origin,
            })
        })
    }
}
impl effects::Effect for Confirm {
    type Source = Source;
    type Query = ();
    type Body = ForkCreateBody;
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
            let fetched = effects::initial_value::<Self>(ctx)?;
            let lock = orch_db::queries::transaction_guard::in_transaction(conn);
            let current = current_review(ctx, conn, caller, body, lock).await?;
            if current.row.plan != fetched.review.row.plan || current.body != fetched.review.body {
                return Err(refused(updates::UpdateError::Expired));
            }
            let actual = StoredMoveBatch {
                recipes: fetched
                    .hosted
                    .iter()
                    .map(|draft| draft.recipe.clone())
                    .collect(),
                portable_recipes: fetched
                    .portable
                    .iter()
                    .map(|draft| draft.recipe.clone())
                    .collect(),
                observations: current.stored.observations.clone(),
                request: current.stored.request.clone(),
                notification: current.stored.notification.clone(),
            };
            let ids = members(&actual);
            let destinations = destinations(&actual);
            let selected = facts(conn, &ids, &destinations).await?;
            let known = native::bind_known(ctx, &CREATE, &selected)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_members(ctx, conn, &principal, &current).await?;
            let mut hosted = Vec::new();
            let mut portable = Vec::new();
            for draft in &fetched.hosted {
                let environment = draft.recipe.replacement.target.id();
                envs::lock_environment(conn, environment)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if envs::live_runtime(conn, environment)
                    .await?
                    .is_none_or(|runtime| runtime.id != draft.recipe.replacement.old)
                {
                    return Err(ApiError::new(
                        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                        "a unit changed after the native move was prepared",
                    ));
                }
                hosted.push(
                    environment_move::prepare_queue(conn, draft.clone())
                        .await
                        .map_err(refused)?,
                );
            }
            for draft in &fetched.portable {
                portable.push(
                    machine_move::prepare_queue(conn, &principal, draft.clone(), ctx.now)
                        .await
                        .map_err(refused)?,
                );
            }
            let under = envs::environments_under(conn, current.node).await?;
            if ids.iter().any(|id| !under.contains(id)) {
                return Err(ApiError::not_found());
            }
            let recipients =
                orch_engine::notifications::operation_audience(conn, &principal, &ids, &[])
                    .await
                    .map_err(ApiError::internal)?;
            if !orch_db::queries::notifications::operations::matches_preview(
                conn,
                current.body.preflight_id,
                &recipients
                    .iter()
                    .map(|person| person.user)
                    .collect::<Vec<_>>(),
            )
            .await?
            {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                    "The people affected changed; review the batch again",
                ));
            }
            Ok((
                Ready {
                    fetched,
                    hosted,
                    portable,
                    recipients,
                    destinations,
                },
                selected,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_members(ctx, conn, principal, &value.fetched.review).await?;
            require_places(conn, principal, &value.destinations).await
        })
    }
}
fn before_error(error: ApiError) -> updates::UpdateError {
    updates::UpdateError::Internal(orch_engine::EngineError::BeforeEffect(Box::new(error)))
}
impl effects::ApplyOwned for Confirm {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Started>>> {
        Box::pin(async move {
            let batch = new_id();
            let deadline = before.deadline();
            let mut first = Some(before);
            let mut made = Vec::new();
            for queue in &value.hosted {
                let environment = queue.recipe().replacement.target.id();
                let row = orch_engine::notifications::operation_scope(
                    Some(batch),
                    environment_move::queue_prepared(
                        &mut tx,
                        queue.clone(),
                        Some(batch),
                        ctx.now,
                        || {
                            if let Some(check) = first.take() {
                                check.check().map_err(before_error)?;
                            }
                            deadline.check().map_err(before_error)
                        },
                    ),
                )
                .await
                .map_err(refused)?;
                environment_move::authority::retain(
                    &mut tx,
                    &row,
                    batch,
                    &value.fetched.origin,
                    principal.caller(),
                    ctx.app.clock.as_ref(),
                )
                .await
                .map_err(ApiError::internal)?;
                ctx.app
                    .engine
                    .retain_node_environment_move_source(&mut tx, &row)
                    .await
                    .map_err(ApiError::engine)?;
                made.push((environment, row.id));
            }
            for queue in &value.portable {
                let environment = queue.recipe().environment;
                let row = orch_engine::notifications::operation_scope(
                    Some(batch),
                    machine_move::queue_prepared(
                        &mut tx,
                        queue.clone(),
                        Some(batch),
                        ctx.now,
                        || {
                            if let Some(check) = first.take() {
                                check.check().map_err(before_error)?;
                            }
                            deadline.check().map_err(before_error)
                        },
                    ),
                )
                .await
                .map_err(refused)?;
                environment_move::authority::retain(
                    &mut tx,
                    &row,
                    batch,
                    &value.fetched.origin,
                    principal.caller(),
                    ctx.app.clock.as_ref(),
                )
                .await
                .map_err(ApiError::internal)?;
                ctx.app
                    .engine
                    .retain_node_environment_move_source(&mut tx, &row)
                    .await
                    .map_err(ApiError::engine)?;
                made.push((environment, row.id));
            }
            if let Some(check) = first.take() {
                check.check()?;
            }
            let operation = orch_engine::notifications::NotificationOperation {
                id: batch,
                kind: "move".into(),
                name: String::new(),
                notify: value.fetched.review.body.notify.unwrap_or(false),
                path: "/resources/environments".into(),
                subject: None,
                recipients: value.recipients.clone(),
            };
            orch_engine::notifications::attach_operation(
                &mut tx,
                &made.iter().map(|(_, id)| *id).collect::<Vec<_>>(),
                &operation,
                ctx.now,
            )
            .await
            .map_err(ApiError::internal)?;
            orch_engine::notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
                .await
                .map_err(ApiError::internal)?;
            jobs::forks::forget_preflight(&mut tx, value.fetched.review.body.preflight_id).await?;
            if Time::new(value.fetched.review.row.expires_at) <= ctx.app.clock.time() {
                return Err(refused(updates::UpdateError::Expired));
            }
            deadline.check()?;
            orch_db::idempotency::creation::observe_with(
                &mut tx,
                Some(ctx.user()?.id()),
                CREATION_KIND,
                batch,
                serde_json::to_value(Accepted {
                    node: value.fetched.review.node,
                    jobs: made.clone(),
                })
                .map_err(ApiError::internal)?,
            )
            .await?;
            if Time::new(value.fetched.review.row.expires_at) <= ctx.app.clock.time() {
                return Err(refused(updates::UpdateError::Expired));
            }
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(Started {
                node: value.fetched.review.node,
                batch,
                jobs: made,
                destinations: value.destinations.clone(),
            }))
        })
    }
}
mod readback;
pub(in crate::routes) async fn create(
    ctx: Ctx,
    (): (),
    _: ForkCreateBody,
) -> ApiResult<api::NodeEnvironmentMoveCreateResponse> {
    let tx = ctx.app.db.begin().await?;
    let done = effects::apply_owned::<Confirm>(&ctx, tx).await?;
    done.ctx.app.engine.kick();
    let receipt = api::NodeEnvironmentMoveCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        node_id: done.output.node,
        batch_id: done.output.batch,
        job_ids: done.output.jobs.iter().map(|(_, id)| *id).collect(),
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<readback::Content>(
        &done.ctx,
        &done.accepted,
        Readback::First(done.output),
    )
    .await?
    {
        Some(full) => Ok(api::NodeEnvironmentMoveCreateResponse::Full(Box::new(full))),
        None => Ok(api::NodeEnvironmentMoveCreateResponse::Receipt(receipt)),
    }
}
