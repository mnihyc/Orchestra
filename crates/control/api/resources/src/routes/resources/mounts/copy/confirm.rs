//! Copy acceptance preserves the reviewed operation and original caller.
use super::*;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::workspace::{Job, JobCreationReceipt, JobCreationResponse};
use orch_engine::jobs::mount_copy;
use serde_json::json;

pub(super) struct Ready {
    stored: Stored,
}
pub(super) struct Confirm;
#[derive(Clone)]
pub(super) struct Started {
    review: Review,
    job: Id,
    actor: Id,
}
impl effects::Effect for Confirm {
    type Source = Source;
    type Query = ();
    type Body = api::MountCopyCreateBody;
    type Value = Arc<Ready>;
    type Output = Started;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &declarations::CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                jobs::changes::lock_plan(conn, body.preflight_id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
            }
            let row = jobs::forks::preflight(conn, body.preflight_id)
                .await?
                .filter(|row| row.kind == "mount_copy" && row.created_by == caller.user())
                .ok_or_else(ApiError::not_found)?;
            let stored: Stored = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
            if stored.review.source().mount != ctx.id("id")? {
                return Err(ApiError::not_found());
            }
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                operation::lock_source(conn, &stored.review)
                    .await
                    .map_err(mapped)?;
            }
            if let Some(job) = stored.job {
                let accepted = jobs::job(conn, job)
                    .await?
                    .filter(|job| {
                        job.kind == mount_copy::KIND && job.created_by == Some(caller.user())
                    })
                    .ok_or_else(ApiError::not_found)?;
                if accepted.plan["mount_id"] != json!(ctx.id("id")?) {
                    return Err(stale());
                }
            } else {
                if row.expires_at <= ctx.app.clock.now()
                    || !stored
                        .review
                        .current(conn)
                        .await
                        .map_err(ApiError::engine)?
                {
                    return Err(stale());
                }
                if !stored.plan.blockers.is_empty()
                    || stored.plan.copy.as_ref().is_none_or(|copy| !copy.measured)
                {
                    return Err(ApiError::new(
                        ErrorCode::PlanBlocked,
                        "The reviewed copy is not ready",
                    ));
                }
            }
            let facts = operation::facts(conn, &stored.review)
                .await
                .map_err(ApiError::engine)?;
            Ok((Arc::new(Ready { stored }), facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            operation::require(conn, principal, &value.stored.review)
                .await
                .map_err(mapped)
        })
    }
}
impl effects::Apply for Confirm {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Started>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            let actor = principal.user();
            if let Some(job) = value.stored.job {
                deadline.check()?;
                return Ok(effects::Outcome::Applied(Started {
                    review: value.stored.review.clone(),
                    job,
                    actor,
                }));
            }
            let captured =
                CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
            let origin = native::http_origin::capture_operation(ctx, conn, &captured).await?;
            let recipe = mount_copy::Recipe {
                review: value.stored.review.clone(),
                decisions: value.stored.decisions.clone(),
                origin,
            };
            let job = ctx
                .app
                .engine
                .queue_mount_copy(conn, &recipe)
                .await
                .map_err(ApiError::engine)?;
            let mut accepted = value.stored.clone();
            accepted.job = Some(job.id);
            sqlx::query("UPDATE preflights SET plan=$2 WHERE id=$1")
                .bind(accepted.plan.preflight_id)
                .bind(serde_json::to_value(&accepted).map_err(ApiError::internal)?)
                .execute(conn)
                .await
                .map_err(DbError::from)?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Started {
                review: accepted.review,
                job: job.id,
                actor,
            }))
        })
    }
}
struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Started;
    type Subject = ();
    type Value = jobs::JobRow;
    type Reply = Job;
    const READ_SOURCE: Source = Source::Mount;
    fn declaration(_: &Started) -> &'static Declaration<Source> {
        &declarations::CREATE
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Started,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            Ok((
                (),
                operation::facts(conn, &input.review)
                    .await
                    .map_err(ApiError::engine)?,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Started,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                ResourceKind::Mount,
                input.review.source().mount,
                Permission::MountView,
            )
            .await?;
            for machine in [input.review.source().machine, input.review.target_machine()] {
                seen(ctx, conn, principal, machine)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Started,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<(jobs::JobRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.job)
                .await?
                .filter(|row| {
                    row.kind == mount_copy::KIND
                        && row.created_by == Some(input.actor)
                        && row.plan["mount_id"] == json!(input.review.source().mount)
                })
                .ok_or_else(ApiError::not_found)?;
            Ok((row, vec![]))
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Started,
        value: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            let mut names = std::collections::HashMap::new();
            for machine in [input.review.source().machine, input.review.target_machine()] {
                if let Some((reference, _)) = seen(ctx, conn, principal, machine).await? {
                    names.insert(machine, reference.name);
                }
            }
            crate::routes::environments::jobs::view_with_machine_sources(
                conn,
                &value.value,
                Some(&names),
            )
            .await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Started,
        old: &'a reply::Prepared<Self>,
        new: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.value.id == new.value.id && old.value.updated_at == new.value.updated_at)
        })
    }
}
pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::MountCopyCreateBody,
) -> ApiResult<JobCreationResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Confirm>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    let receipt = JobCreationReceipt {
        projection: ReceiptProjection::Receipt,
        job_id: done.output.job,
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await? {
        Some(job) => Ok(JobCreationResponse::Full(Box::new(job))),
        None => Ok(JobCreationResponse::Receipt(receipt)),
    }
}
const REPLAY_KIND: &str = "mount_copy_job";
pub(super) fn replay_binding()
-> crate::middleware::idempotency::Creation<(), api::MountCopyCreateBody, JobCreationResponse> {
    crate::middleware::idempotency::Creation {
        kind: REPLAY_KIND,
        replay,
        identity: |value| {
            let id = match value {
                JobCreationResponse::Full(job) => job.id,
                JobCreationResponse::Receipt(receipt) => receipt.job_id,
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: REPLAY_KIND.into(),
                id,
                data: Json::Null,
            })
        },
    }
}
fn replay(
    ctx: Ctx,
    (): (),
    _: api::MountCopyCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> BoxFuture<'static, ApiResult<JobCreationResponse>> {
    Box::pin(async move {
        if created.kind != REPLAY_KIND {
            return Err(ApiError::internal(
                "The accepted copy has another creation kind",
            ));
        }
        let actor = ctx.user()?.id();
        let mut conn = ctx.app.db.acquire().await?;
        let row = jobs::job(&mut conn, created.id)
            .await?
            .filter(|row| row.kind == mount_copy::KIND && row.created_by == Some(actor))
            .ok_or_else(ApiError::not_found)?;
        let recipe: mount_copy::Recipe =
            serde_json::from_value(row.plan["copy"].clone()).map_err(ApiError::internal)?;
        if recipe.review.source().mount != ctx.id("id")? {
            return Err(ApiError::not_found());
        }
        drop(conn);
        let receipt = JobCreationReceipt {
            projection: ReceiptProjection::Receipt,
            job_id: created.id,
        };
        publication::receipt(&ctx, &receipt)?;
        match reply::render_completed_replay::<Content>(
            &ctx,
            actor,
            Started {
                review: recipe.review,
                job: created.id,
                actor,
            },
        )
        .await
        {
            Ok(Some(job)) => Ok(JobCreationResponse::Full(Box::new(job))),
            Ok(None) => Ok(JobCreationResponse::Receipt(receipt)),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(JobCreationResponse::Receipt(receipt))
            }
            Err(error) => Err(error),
        }
    })
}
