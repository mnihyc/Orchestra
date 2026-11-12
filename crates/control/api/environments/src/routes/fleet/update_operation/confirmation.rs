//! Confirm the native stored review once, with fresh locked recipes and retained worker authority.

use super::*;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::forks::UpdateConfirmQuery;

use orch_engine::jobs::recreate;

use orch_engine::jobs::recreate::PreparedQueue;
const CREATION_KIND: &str = "node_environment_update_batch";

#[derive(serde::Serialize, serde::Deserialize)]
struct Accepted {
    node: Id,
    jobs: Vec<(Id, Id)>,
}

enum Readback {
    First(Box<Started>),
    Replay { batch: Id, accepted: Accepted },
}

impl Readback {
    fn batch(&self) -> Id {
        match self {
            Self::First(started) => started.batch,
            Self::Replay { batch, .. } => *batch,
        }
    }
    fn node(&self) -> Id {
        match self {
            Self::First(started) => started.node,
            Self::Replay { accepted, .. } => accepted.node,
        }
    }
    fn jobs(&self) -> &[(Id, Id)] {
        match self {
            Self::First(started) => &started.jobs,
            Self::Replay { accepted, .. } => &accepted.jobs,
        }
    }
    fn ids(&self) -> Vec<Id> {
        self.jobs().iter().map(|(id, _)| *id).collect()
    }
    async fn required(&self, conn: &mut Conn) -> ApiResult<Vec<native::Facts<Source>>> {
        match self {
            Self::First(started) => required_facts(conn, &self.ids(), &started.templates).await,
            Self::Replay { .. } => Ok(Vec::new()),
        }
    }
}

pub(in crate::routes::fleet) fn binding() -> crate::middleware::idempotency::Creation<
    UpdateConfirmQuery,
    ForkCreateBody,
    api::NodeEnvironmentUpdateCreateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: CREATION_KIND,
        identity: |response| {
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION_KIND.into(),
                id: match response {
                    api::NodeEnvironmentUpdateCreateResponse::Full(value) => value.batch_id,
                    api::NodeEnvironmentUpdateCreateResponse::Receipt(value) => value.batch_id,
                },
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, _, _, created| {
            Box::pin(async move {
                if created.kind != CREATION_KIND {
                    return Err(ApiError::internal("the accepted native batch kind changed"));
                }
                let accepted: Accepted =
                    serde_json::from_value(created.data).map_err(ApiError::internal)?;
                if ctx.id("id")? != accepted.node {
                    return Err(ApiError::not_found());
                }
                let receipt = api::NodeEnvironmentUpdateCreateReceipt {
                    projection: ReceiptProjection::Receipt,
                    node_id: accepted.node,
                    batch_id: created.id,
                    job_ids: accepted.jobs.iter().map(|(_, job)| *job).collect(),
                };
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                match native::reply::render_completed_replay::<readback::Reply>(
                    &ctx,
                    ctx.user()?.id(),
                    Readback::Replay {
                        batch: created.id,
                        accepted,
                    },
                )
                .await
                {
                    Ok(Some(full)) => Ok(api::NodeEnvironmentUpdateCreateResponse::Full(Box::new(
                        full,
                    ))),
                    Ok(None) => Ok(api::NodeEnvironmentUpdateCreateResponse::Receipt(receipt)),
                    Err(error)
                        if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) =>
                    {
                        Ok(api::NodeEnvironmentUpdateCreateResponse::Receipt(receipt))
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(in crate::routes::fleet) struct Confirm;

#[derive(Clone)]
pub(in crate::routes::fleet) struct Review {
    node: Id,
    body: ForkCreateBody,
    query: UpdateConfirmQuery,
    recipes: Vec<updates::Recipe>,
    templates: Vec<Id>,
    expires_at: Time,
}

pub(in crate::routes::fleet) struct Prepared {
    review: Review,
    queues: Vec<PreparedQueue>,
    sources: BTreeMap<Id, NativeInput>,
    accepted: Caller,
    until: Option<Time>,
}

pub(in crate::routes::fleet) struct Started {
    node: Id,
    batch: Id,
    jobs: Vec<(Id, Id)>,
    templates: Vec<Id>,
    accepted: Caller,
    until: Option<Time>,
}

impl Started {
    fn receipt(&self) -> api::NodeEnvironmentUpdateCreateReceipt {
        api::NodeEnvironmentUpdateCreateReceipt {
            projection: ReceiptProjection::Receipt,
            node_id: self.node,
            batch_id: self.batch,
            job_ids: self.jobs.iter().map(|(_, job)| *job).collect(),
        }
    }
}

fn row() -> &'static Declaration<Source> {
    &orch_api_contract::environment_sources::update_batch::NODE_CREATE
}

fn ids(review: &Review) -> Vec<Id> {
    review
        .recipes
        .iter()
        .map(|recipe| recipe.target.id())
        .collect()
}

async fn review(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: UpdateConfirmQuery,
    body: ForkCreateBody,
    lock: bool,
) -> ApiResult<Review> {
    let node = ctx.id("id")?;
    let known = native::bind_known(ctx, row(), &[])?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    node_view(conn, &principal, Some(node)).await?;
    let (stored, recipes) =
        plans::reviewed_batch_row(ctx, conn, &principal, Some(node), false, &body, lock).await?;
    let under = envs::environments_under(conn, node).await?;
    if recipes
        .iter()
        .any(|recipe| !under.contains(&recipe.target.id()))
    {
        return Err(ApiError::not_found());
    }
    let mut templates = std::collections::BTreeSet::new();
    for recipe in &recipes {
        if let Some((selected, old)) =
            updates::environment_update_template(conn, recipe.target.id())
                .await
                .map_err(source_error)?
            && selected != old
        {
            templates.insert(selected);
        }
    }
    Ok(Review {
        node,
        body,
        query,
        recipes,
        templates: templates.into_iter().collect(),
        expires_at: Time::new(stored.expires_at),
    })
}

pub(in crate::routes::fleet) async fn required_facts(
    conn: &mut Conn,
    ids: &[Id],
    templates: &[Id],
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut selected = Vec::new();
    for id in templates {
        selected.push(
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Template,
                    id: *id,
                },
            )
            .await?,
        );
    }
    Ok(vec![
        environment_facts(conn, Source::Environments, ids).await?,
        native::Facts::collection(
            Source::Templates,
            if selected.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Template), selected)]
            },
        ),
    ])
}

fn review_deadline(ctx: &Ctx, review: &Review) -> ApiResult<()> {
    if ctx.app.clock.time() >= review.expires_at {
        return Err(plans::refused(UpdateError::Expired));
    }
    Ok(())
}

async fn require_review(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    review: &Review,
) -> ApiResult<()> {
    review_deadline(ctx, review)?;
    node_view(conn, principal, Some(review.node)).await?;
    for recipe in &review.recipes {
        if !updates::may_manage(conn, principal, recipe.target)
            .await
            .map_err(source_error)?
        {
            return Err(plans::refused(UpdateError::Denied));
        }
    }
    Ok(())
}

impl Effect for Confirm {
    type Source = Source;
    type Query = UpdateConfirmQuery;
    type Body = ForkCreateBody;
    type Value = Prepared;
    type Output = Started;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        row()
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let initial = effects::initial_value::<Self>(ctx)?;
            let mut accepted = caller
                .intersect_restrictions(&initial.accepted)
                .map_err(ApiError::internal)?;
            retained_deadline(ctx, &accepted, initial.until)?;
            let guarded = orch_db::queries::transaction_guard::in_transaction(conn);
            let review = review(ctx, conn, &accepted, query, body, guarded).await?;
            if guarded {
                for source in initial.sources.values() {
                    source.lock(conn).await.map_err(source_error)?;
                }
            }
            let facts = required_facts(conn, &ids(&review), &review.templates).await?;
            let known = native::bind_known(ctx, row(), &facts)?;
            let principal = native::preparation_principal(ctx, conn, &accepted, &known).await?;
            require_review(ctx, conn, &principal, &review).await?;
            let mut queues = Vec::new();
            for recipe in &review.recipes {
                let observed = initial
                    .observations
                    .get(&recipe.target.id())
                    .ok_or_else(|| plans::refused(UpdateError::Expired))?;
                let draft = ctx
                    .app
                    .engine
                    .replay_update(
                        conn,
                        &principal,
                        recipe.target,
                        api::UpdatePreflightReason::Update,
                        review.body.options.as_ref(),
                        observed,
                    )
                    .await
                    .map_err(update_error)?;
                plans::check_reviewed_draft(recipe, &draft, false)?;
                queues.push(
                    recreate::prepare_queue(conn, draft)
                        .await
                        .map_err(update_error)?,
                );
            }
            let mut until = initial.until;
            let mut sources = BTreeMap::new();
            for queue in &queues {
                let id = queue.recipe().target.id();
                let mut source = initial
                    .sources
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| plans::refused(UpdateError::Expired))?;
                source
                    .narrow(conn, ctx.app.clock.as_ref(), &accepted, until)
                    .await
                    .map_err(source_error)?;
                if guarded {
                    review_deadline(ctx, &review)?;
                    let check = source
                        .checkpoint(conn, ctx.app.clock.as_ref())
                        .await
                        .map_err(source_error)?;
                    if !check.matches() {
                        return Err(plans::refused(UpdateError::Expired));
                    }
                    accepted = accepted
                        .intersect_restrictions(check.principal().caller())
                        .map_err(ApiError::internal)?;
                    until = minimum(until, check.until);
                    source = check.accepted_input().map_err(source_error)?;
                }
                sources.insert(id, source.for_execution(queue.recipe().template));
            }
            for source in sources.values_mut() {
                source
                    .narrow(conn, ctx.app.clock.as_ref(), &accepted, until)
                    .await
                    .map_err(source_error)?;
            }
            let principal = native::preparation_principal(ctx, conn, &accepted, &known).await?;
            require_review(ctx, conn, &principal, &review).await?;
            retained_deadline(ctx, &accepted, until)?;
            Ok((
                Prepared {
                    review,
                    queues,
                    sources,
                    accepted,
                    until,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let facts = required_facts(conn, &ids(&value.review), &value.review.templates).await?;
            let known = native::bind_known(ctx, row(), &facts)?;
            let principal =
                native::preparation_principal(ctx, conn, &value.accepted, &known).await?;
            require_review(ctx, conn, &principal, &value.review).await?;
            retained_deadline(ctx, principal.caller(), value.until)
        })
    }
}

impl Initial for Confirm {
    type Input = Review;
    type Fetched = Fetched;
    fn declaration(_: &Review) -> &'static Declaration<Source> {
        row()
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: Self::Query,
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Review, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let review = review(ctx, conn, caller, query, body, false).await?;
            let facts = required_facts(conn, &ids(&review), &review.templates).await?;
            Ok((review, facts))
        })
    }
    fn require_initial<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Review,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { require_review(ctx, conn, principal, input).await })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        review: Review,
    ) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let ids = ids(&review);
            let selected = Selection {
                node: Some(review.node),
                visible: ids.clone(),
                managed: ids,
            };
            let expected = review
                .recipes
                .iter()
                .map(|recipe| (recipe.target.id(), recipe.old))
                .collect();
            observe(
                ctx,
                caller,
                &selected,
                review.body.options.as_ref(),
                Some(&expected),
                Some(review.body.preflight_id),
            )
            .await
        })
    }
}

fn before_error(error: ApiError) -> UpdateError {
    UpdateError::Internal(orch_engine::EngineError::BeforeEffect(Box::new(error)))
}

impl ApplyOwned for Confirm {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        _: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Started>>> {
        Box::pin(async move {
            let batch = new_id();
            let deadline = before.deadline();
            let mut first = Some(before);
            let mut jobs = Vec::new();
            let mut threads = Vec::new();
            for queue in &value.queues {
                let environment = queue.recipe().target.id();
                threads.extend(queue.recipe().threads.iter().copied());
                let job =
                    recreate::queue_prepared(&mut tx, queue.clone(), Some(batch), ctx.now, || {
                        if let Some(first) = first.take() {
                            first.check().map_err(before_error)?;
                        }
                        review_deadline(ctx, &value.review).map_err(before_error)?;
                        retained_deadline(ctx, &value.accepted, value.until).map_err(before_error)
                    })
                    .await
                    .map_err(update_error)?;
                let source = value.sources.get(&environment).ok_or_else(|| {
                    ApiError::internal("the prepared replacement source disappeared")
                })?;
                recreate::batch_authority::environment::retain(
                    &mut tx,
                    job.id,
                    batch,
                    source,
                    Time::new(ctx.now),
                )
                .await
                .map_err(source_error)?;
                jobs.push((environment, job.id));
            }
            if let Some(first) = first.take() {
                first.check()?;
            }
            retained_deadline(ctx, &value.accepted, value.until)?;
            review_deadline(ctx, &value.review)?;
            if value.review.query.notify.unwrap_or(true) {
                plans::tell_update(ctx, &mut tx, &threads, batch).await?;
            }
            orch_db::queries::jobs::forks::forget_preflight(
                &mut tx,
                value.review.body.preflight_id,
            )
            .await?;
            deadline.check()?;
            retained_deadline(ctx, &value.accepted, value.until)?;
            review_deadline(ctx, &value.review)?;
            orch_db::idempotency::creation::observe_with(
                &mut tx,
                Some(ctx.user()?.id()),
                CREATION_KIND,
                batch,
                serde_json::to_value(Accepted {
                    node: value.review.node,
                    jobs: jobs.clone(),
                })
                .map_err(ApiError::internal)?,
            )
            .await?;
            deadline.check()?;
            retained_deadline(ctx, &value.accepted, value.until)?;
            review_deadline(ctx, &value.review)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(Outcome::Applied(Started {
                node: value.review.node,
                batch,
                jobs,
                templates: value.review.templates.clone(),
                accepted: value.accepted.clone(),
                until: value.until,
            }))
        })
    }
}

pub(in crate::routes::fleet) async fn create(
    ctx: Ctx,
    _: UpdateConfirmQuery,
    _: ForkCreateBody,
) -> ApiResult<api::NodeEnvironmentUpdateCreateResponse> {
    let tx = ctx.app.db.begin().await?;
    let done = effects::apply_owned::<Confirm>(&ctx, tx).await?;
    done.ctx.app.engine.kick();
    let receipt = done.output.receipt();
    crate::key_operations::publication::receipt(&done.ctx, &receipt)?;
    let reader = done
        .accepted
        .intersect_restrictions(&done.output.accepted)
        .map_err(ApiError::internal)?;
    done.ctx.publication.retain_deadline(done.output.until)?;
    match native::reply::render::<readback::Reply>(
        &done.ctx,
        &reader,
        Readback::First(Box::new(done.output)),
    )
    .await?
    {
        Some(full) => Ok(api::NodeEnvironmentUpdateCreateResponse::Full(Box::new(
            full,
        ))),
        None => Ok(api::NodeEnvironmentUpdateCreateResponse::Receipt(receipt)),
    }
}

#[path = "confirmation/readback.rs"]
pub mod readback;
