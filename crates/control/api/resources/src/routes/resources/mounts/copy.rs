//! Standalone data copies use the shared mount planner and durable jobs.
use super::*;
use crate::{
    key_operations::{
        native::{self, effects, reply},
        publication,
    },
    middleware::transaction_guard,
};
use futures::future::BoxFuture;
use orch_api_contract::{
    mount_copy_sources::{self as declarations, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, CapturedOperation};
use orch_db::queries::jobs;
use orch_engine::mount_copies::{
    self,
    location::{Review, operation, preparation::Preparation},
};
use orch_types::new_id;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            api::MountCopyLatest,
            native::selected::<latest::Latest>,
            native::selected_read::<latest::Latest>,
        )
        .on_with_initial_operation(
            api::MountCopyPreflight,
            effects::initial::<Preview>,
            effects::prepare::<Preview>,
            preview,
        )
        .on_with_operation(
            api::MountCopyCreate,
            effects::prepare::<confirm::Confirm>,
            confirm::run,
        )
        .creation(api::MountCopyCreate, confirm::replay_binding());
}

#[path = "copy/confirm.rs"]
mod confirm;
#[path = "copy/latest.rs"]
mod latest;

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    review: Review,
    body: api::MountCopyPreflightBody,
    plan: api::MountCopyPlan,
    decisions: mount_copies::MountDecisions,
    job: Option<Id>,
}

fn mapped(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match error {
        orch_engine::operation_scope::continuing::Error::Native(error) => ApiError::engine(error),
        _ => ApiError::new(
            ErrorCode::AuthzDenied,
            "The original mount copy no longer permits this work",
        ),
    }
}
fn stale() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "The mount location or its data storage changed; review the copy again",
    )
}

struct Input {
    mount: Id,
    id: Id,
    body: api::MountCopyPreflightBody,
}
struct Preview;
impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = api::MountCopyPreflightBody;
    type Value = Arc<Stored>;
    type Output = Arc<Stored>;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &declarations::PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        _: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let stored = effects::initial_value::<Self>(ctx)?;
            if ctx.id("id")? != stored.review.source().mount {
                return Err(ApiError::not_found());
            }
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                operation::lock_source(conn, &stored.review)
                    .await
                    .map_err(mapped)?;
            }
            if !stored
                .review
                .current(conn)
                .await
                .map_err(ApiError::engine)?
            {
                return Err(stale());
            }
            if stored.plan.expires_at <= ctx.app.clock.time() {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "The copy review expired",
                ));
            }
            let facts = operation::facts(conn, &stored.review)
                .await
                .map_err(ApiError::engine)?;
            Ok((stored, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            operation::require(conn, principal, &value.review)
                .await
                .map_err(mapped)
        })
    }
}
impl effects::Initial for Preview {
    type Input = Input;
    type Fetched = Stored;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &declarations::PREFLIGHT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let mount = ctx.id("id")?;
            if body.source_machine_id == body.target_machine_id {
                return Err(ApiError::invalid(
                    "target_machine_id",
                    "choose another machine",
                ));
            }
            let facts =
                operation::subjects(conn, mount, body.source_machine_id, body.target_machine_id)
                    .await
                    .map_err(ApiError::engine)?;
            Ok((
                Input {
                    mount,
                    id: new_id(),
                    body,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for machine in [input.body.source_machine_id, input.body.target_machine_id] {
                let ask = LocationAsk {
                    mount: input.mount,
                    machine,
                    kind: LocationKind::Path,
                    storage: None,
                };
                if !matches!(
                    may_add_location(conn, principal, &ask, &Policies::default())
                        .await
                        .map_err(ApiError::internal)?,
                    Locating::Allowed
                ) {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "Management of both copy machines and use of the mount are required",
                    ));
                }
            }
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Stored>> {
        Box::pin(async move {
            let (review, origin, principal) = {
                let mut tx = ctx.app.db.begin_snapshot().await?;
                let review = Review::select(
                    &mut tx,
                    input.mount,
                    input.body.source_machine_id,
                    input.body.target_machine_id,
                )
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::PlanBlocked,
                        "The source location or a machine's data storage is unavailable",
                    )
                })?;
                let facts = operation::facts(&mut tx, &review)
                    .await
                    .map_err(ApiError::engine)?;
                let known = native::bind_known(ctx, &declarations::PREFLIGHT, &facts)?;
                let principal = native::preparation_principal(ctx, &mut tx, caller, &known).await?;
                let captured =
                    CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
                let origin =
                    native::http_origin::capture_operation(ctx, &mut tx, &captured).await?;
                operation::require(&mut tx, &principal, &review)
                    .await
                    .map_err(mapped)?;
                tx.commit().await.map_err(DbError::from)?;
                (review, origin, principal)
            };
            let preparation = Preparation::new(review.clone(), origin).map_err(ApiError::engine)?;
            let decided = {
                let mut conn = ctx.app.db.acquire().await?;
                preparation
                    .scope(mount_copies::decide_location_copy(
                        &ctx.app.engine,
                        &mut conn,
                        &principal,
                        &review.source().row(),
                        input.body.target_machine_id,
                        input.body.into,
                        ctx.now,
                    ))
                    .await
                    .map_err(ApiError::engine)?
            };
            let mut tx = ctx.app.db.begin_snapshot().await?;
            let current =
                native::http_origin::current(ctx, &mut tx, &preparation.retained()).await?;
            let principal =
                Principal::load(&mut tx, current.caller().clone(), ctx.app.clock.time())
                    .await
                    .map_err(ApiError::internal)?;
            if !review.current(&mut tx).await.map_err(ApiError::engine)? {
                return Err(stale());
            }
            let mount = rows::mounts(&mut tx, &[input.mount])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let mut machines = machine_refs(
                &ctx.app,
                &mut tx,
                &[input.body.source_machine_id, input.body.target_machine_id],
            )
            .await?;
            let seconds = orch_settings::get::<i64>(
                &mut tx,
                mount
                    .home_id
                    .map_or(orch_settings::Place::System, orch_settings::Place::Node),
                orch_settings_contract::registry::keys::MOUNT_COPY_REVIEW_SECS,
            )
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(900);
            let expires_at = chrono::Duration::try_seconds(seconds)
                .filter(|_| seconds > 0)
                .and_then(|span| ctx.app.clock.now().checked_add_signed(span))
                .ok_or_else(|| {
                    ApiError::invalid(
                        "mount.copy_review_secs",
                        "the review time must be positive and produce a valid expiry date",
                    )
                })?;
            let plan = api::MountCopyPlan {
                preflight_id: input.id,
                expires_at: Time::new(expires_at),
                mount: MountRef {
                    id: mount.id,
                    name: mount.name,
                    address: String::new(),
                },
                source: machines
                    .remove(&input.body.source_machine_id)
                    .ok_or_else(ApiError::not_found)?,
                target: machines
                    .remove(&input.body.target_machine_id)
                    .ok_or_else(ApiError::not_found)?,
                copy: decided.fated.first().and_then(|row| row.copy.clone()),
                blockers: decided.blockers,
            };
            native::http_origin::check(ctx, &current)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(Stored {
                review,
                body: input.body,
                plan,
                decisions: decided.decisions,
                job: None,
            })
        })
    }
}
impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            jobs::forks::save_preflight(
                conn,
                &jobs::forks::PreflightRow {
                    id: value.plan.preflight_id,
                    kind: "mount_copy".into(),
                    node_id: None,
                    environment_id: None,
                    created_by: principal.user(),
                    plan: serde_json::to_value(value.as_ref()).map_err(ApiError::internal)?,
                    expires_at: value.plan.expires_at.get(),
                    created_at: ctx.now,
                },
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(value.clone()))
        })
    }
}

struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Arc<Stored>;
    type Subject = ();
    type Value = ();
    type Reply = api::MountCopyPlan;
    const READ_SOURCE: Source = Source::Mount;
    fn declaration(_: &Self::Input) -> &'static Declaration<Source> {
        &declarations::PREFLIGHT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
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
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            operation::require(conn, principal, &input.review)
                .await
                .map_err(mapped)
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Input,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), vec![])) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Self::Input,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Self::Reply>> {
        Box::pin(async move {
            let mut plan = input.plan.clone();
            require(
                conn,
                principal,
                ResourceKind::Mount,
                plan.mount.id,
                Permission::MountView,
            )
            .await?;
            plan.source = seen(ctx, conn, principal, plan.source.id)
                .await?
                .ok_or_else(ApiError::not_found)?
                .0;
            plan.target = seen(ctx, conn, principal, plan.target.id)
                .await?
                .ok_or_else(ApiError::not_found)?
                .0;
            Ok(plan)
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Self::Input,
        _: &'a reply::Prepared<Self>,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.plan.preflight_id).await?;
            let expected = serde_json::to_value(input.as_ref()).map_err(ApiError::internal)?;
            Ok(
                row.is_some_and(|row| row.kind == "mount_copy" && row.plan == expected)
                    && input.review.current(conn).await.map_err(ApiError::engine)?,
            )
        })
    }
}

async fn preview(
    ctx: Ctx,
    (): (),
    _: api::MountCopyPreflightBody,
) -> ApiResult<api::MountCopyPlanResponse> {
    let (mut tx, _) = transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::MountCopyPlanReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        mount_id: done.output.review.source().mount,
        preflight_id: done.output.plan.preflight_id,
        expires_at: done.output.plan.expires_at,
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await? {
        Some(plan) => Ok(api::MountCopyPlanResponse::Full(Box::new(plan))),
        None => Ok(api::MountCopyPlanResponse::Receipt(receipt)),
    }
}
