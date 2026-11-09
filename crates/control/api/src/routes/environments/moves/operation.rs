//! One real move observation, native saved-review effect and independently admitted readback.
use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication,
};
use crate::routes::environments::lifecycle as common;
use futures::future::BoxFuture;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::{
    environment_sources::lifecycle::{MOVE_PREFLIGHT, Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_engine::jobs::environment_move::preparation::{self, Kind, Origin};
use std::sync::Arc;

pub(super) struct Preview;
pub(super) struct Input {
    environment: Id,
    body: api::EnvironmentMovePreflightBody,
}
#[derive(Clone)]
pub(super) enum Draft {
    Hosted(Box<MoveDraft>),
    Portable(Box<machine_move::Draft>),
}
pub(super) struct Observed {
    origin: Origin,
    id: Id,
    body: api::EnvironmentMovePreflightBody,
    draft: Draft,
    plan: EnvironmentMovePlan,
    audience: Vec<orch_engine::notifications::OperationRecipient>,
    visible: Vec<Id>,
}
impl common::Observed for Observed {
    fn origin(&self) -> &Origin {
        &self.origin
    }
}
impl Observed {
    fn extra(&self) -> Vec<(ObjectKind, Id)> {
        let (threads, mounts, mcp) = match &self.draft {
            Draft::Hosted(d) => (&d.recipe.replacement.threads, &d.mounts, &d.mcp),
            Draft::Portable(d) => (&d.recipe.threads, &d.mounts, &d.mcp),
        };
        threads
            .iter()
            .map(|id| (ObjectKind::Thread, *id))
            .chain(mounts.iter().map(|m| (ObjectKind::Mount, m.mount.id)))
            .chain(
                mcp.iter()
                    .filter(|s| !s.own && !s.imported)
                    .map(|s| (ObjectKind::McpServer, s.id)),
            )
            .collect()
    }
}
impl effects::Initial for Preview {
    type Input = Input;
    type Fetched = Observed;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &MOVE_PREFLIGHT.declaration
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let facts = common::objects(
                conn,
                Source::Environments,
                [(ObjectKind::Environment, environment)],
            )
            .await?;
            Ok((Input { environment, body }, vec![facts]))
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
                Kind::MovePreflight,
                &serde_json::to_value(&input.body).map_err(ApiError::internal)?,
                &[input.environment],
                None,
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
            let current = scope
                .current(&ctx.app.engine)
                .await
                .map_err(common::error)?;
            let mut tx = ctx.app.db.begin().await?;
            let principal =
                authorization::load_principal(&mut tx, current, ctx.app.clock.time()).await?;
            let portable = machine_move::needed(&mut tx, input.environment, &input.body)
                .await
                .map_err(ApiError::engine)?;
            tx.rollback().await.map_err(DbError::from)?;
            let draft = preparation::scope(&scope, async {
                if portable {
                    ctx.app
                        .engine
                        .draft_machine_move(&principal, input.environment, &input.body, None)
                        .await
                        .map(Box::new)
                        .map(Draft::Portable)
                } else {
                    let mut tx = ctx
                        .app
                        .db
                        .begin()
                        .await
                        .map_err(orch_engine::EngineError::from)?;
                    let result = ctx
                        .app
                        .engine
                        .draft_move(&mut tx, &principal, input.environment, &input.body, None)
                        .await;
                    tx.rollback()
                        .await
                        .map_err(DbError::from)
                        .map_err(orch_engine::EngineError::from)?;
                    result.map(Box::new).map(Draft::Hosted)
                }
            })
            .await
            .map_err(refused)?;
            let current = scope
                .current(&ctx.app.engine)
                .await
                .map_err(common::error)?;
            let mut conn = ctx.app.db.acquire().await?;
            let principal =
                authorization::load_principal(&mut conn, current, ctx.app.clock.time()).await?;
            let id = new_id();
            let plan = match &draft {
                Draft::Hosted(d) => shown(ctx, &mut conn, &principal, id, d).await?,
                Draft::Portable(d) => shown_machine(ctx, &mut conn, &principal, id, d).await?,
            };
            let audience = orch_engine::notifications::operation_audience(
                &mut conn,
                &principal,
                &[input.environment],
                &[],
            )
            .await
            .map_err(ApiError::engine)?;
            let ids = match &draft {
                Draft::Hosted(d) => &d.recipe.replacement.threads,
                Draft::Portable(d) => &d.recipe.threads,
            };
            let visible = common::visible_threads(&mut conn, &principal, ids).await?;
            Ok(Observed {
                origin: scope.origin().await,
                visible,
                id,
                body: input.body,
                draft,
                plan,
                audience,
            })
        })
    }
}
impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = api::EnvironmentMovePreflightBody;
    type Value = Arc<Observed>;
    type Output = Arc<Observed>;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &MOVE_PREFLIGHT.declaration
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        _: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let observed = effects::initial_value::<Self>(ctx)?;
            if observed.origin.environments().next() != Some(ctx.id("id")?)
                || observed.plan.expires_at <= ctx.app.clock.time()
            {
                return Err(refused(updates::UpdateError::Expired));
            }
            common::current(ctx, conn, &observed.origin).await?;
            let facts = observed
                .origin
                .facts(conn, true)
                .await
                .map_err(common::source_error)?;
            Ok((observed, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { value.origin.require(conn, p).await.map_err(common::error) })
    }
}
impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        value: &'a Self::Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let plan = match &value.draft {
                Draft::Hosted(d) => {
                    json!({"recipe":d.recipe,"request":value.body,"notification":value.plan.notification})
                }
                Draft::Portable(d) => {
                    json!({"portable":d.recipe,"request":value.body,"notification":value.plan.notification})
                }
            };
            let deadline = before.deadline();
            before.check()?;
            jobs::forks::save_preflight(
                conn,
                &PreflightRow {
                    id: value.id,
                    kind: "environment_move".into(),
                    node_id: None,
                    environment_id: value.origin.environments().next(),
                    created_by: p.user(),
                    plan,
                    expires_at: value.plan.expires_at.get(),
                    created_at: ctx.now,
                },
            )
            .await?;
            orch_db::queries::notifications::operations::preview(
                conn,
                value.id,
                &value.audience.iter().map(|p| p.user).collect::<Vec<_>>(),
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Arc::clone(value)))
        })
    }
}
struct Content;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Arc<Observed>;
    type Subject = ();
    type Value = ();
    type Reply = EnvironmentMovePlan;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Self::Input) -> &'static Declaration<Source> {
        &MOVE_PREFLIGHT.declaration
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let saved = jobs::forks::preflight(conn, input.id)
                .await?
                .filter(|r| {
                    r.kind == "environment_move" && r.created_by == input.origin.captured().user()
                })
                .ok_or_else(ApiError::not_found)?;
            if saved.environment_id != input.origin.environments().next()
                || !input
                    .origin
                    .same_source(conn, ctx.app.clock.as_ref())
                    .await
                    .map_err(common::error)?
            {
                return Err(ApiError::not_found());
            }
            let mut facts = input
                .origin
                .facts(conn, true)
                .await
                .map_err(common::source_error)?;
            facts.push(common::content(conn, &input.origin, input.extra()).await?);
            Ok(((), facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Self::Input,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            input.origin.require(conn, p).await.map_err(common::error)?;
            for id in input.origin.environments() {
                super::super::reading::decide(conn, p, id).await?;
            }
            common::require_names(conn, p, &input.visible).await?;
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Input,
        _: &'a (),
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Self::Input,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<EnvironmentMovePlan>> {
        Box::pin(async move {
            match &input.draft {
                Draft::Hosted(d) => shown(ctx, conn, p, input.id, d).await,
                Draft::Portable(d) => shown_machine(ctx, conn, p, input.id, d).await,
            }
        })
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Self::Input,
        _: &'a reply::Prepared<Self>,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            input
                .origin
                .same_source(conn, ctx.app.clock.as_ref())
                .await
                .map_err(common::error)
        })
    }
}
pub(super) async fn preflight(
    ctx: Ctx,
    (): (),
    _: api::EnvironmentMovePreflightBody,
) -> ApiResult<api::EnvironmentMovePreflightResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::EnvironmentMovePreflightReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done
            .output
            .origin
            .environments()
            .next()
            .ok_or_else(ApiError::not_found)?,
        preflight_id: done.output.id,
        expires_at: done.output.plan.expires_at,
    };
    publication::receipt(&done.ctx, &receipt)?;
    Ok(
        match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await? {
            Some(full) => api::EnvironmentMovePreflightResponse::full(full),
            None => api::EnvironmentMovePreflightResponse::Receipt(receipt),
        },
    )
}
