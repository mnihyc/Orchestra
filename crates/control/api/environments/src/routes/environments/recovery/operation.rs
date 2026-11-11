//! Native mixed recovery observations, one saved review and same-operation disclosure.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::routes::environments::lifecycle as common;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::environment_sources::lifecycle::RECOVER_PREFLIGHT;

use orch_api_contract::environment_sources::lifecycle::Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_engine::jobs::environment_move::preparation;

use orch_engine::jobs::environment_move::preparation::Kind;

use orch_engine::jobs::environment_move::preparation::Origin;

use std::sync::Arc;
pub(super) struct Preview;

pub(super) struct Input {
    body: api::EnvironmentRecoverPreflightBody,
    managed: Vec<Id>,
}

pub(super) struct Observed {
    pub(super) origin: Origin,
    pub(super) visible: Vec<Id>,
    pub(super) content: Vec<(ObjectKind, Id)>,
    pub(super) stored: Stored,
    pub(super) plan: api::EnvironmentRecoveryPlan,
    pub(super) audience: Vec<orch_engine::notifications::OperationRecipient>,
}

impl common::Observed for Observed {
    fn origin(&self) -> &Origin {
        &self.origin
    }
}

impl Observed {
    fn extra(&self) -> Vec<(ObjectKind, Id)> {
        let mut objects = self.content.clone();
        for member in &self.stored.members {
            match member {
                Member::Recreate { .. } => {}
                Member::Move { recipe, .. } => objects.extend(
                    recipe
                        .replacement
                        .threads
                        .iter()
                        .map(|id| (ObjectKind::Thread, *id)),
                ),
                Member::Portable { recipe, .. } => {
                    objects.extend(recipe.threads.iter().map(|id| (ObjectKind::Thread, *id)))
                }
            }
        }
        if let Some(switches) = &self.stored.switches {
            objects.extend(
                switches
                    .members
                    .iter()
                    .map(|m| (ObjectKind::Thread, m.thread)),
            );
        }
        objects
    }
}

impl effects::Initial for Preview {
    type Input = Input;
    type Fetched = Observed;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &RECOVER_PREFLIGHT.declaration
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            if body.environment_ids.is_empty() {
                return Err(crate::routes::nodes::invalid(
                    "environment_ids",
                    "select at least one environment",
                ));
            }
            let mut ids = body.environment_ids.clone();
            ids.sort_unstable();
            ids.dedup();
            let mut existing = Vec::new();
            for id in ids {
                if orch_db::queries::environments::hosting::environment(conn, id)
                    .await?
                    .is_some_and(|r| r.deleted_at.is_none())
                {
                    existing.push(id);
                }
            }
            let facts = vec![
                common::objects(
                    conn,
                    Source::Environments,
                    existing.iter().map(|id| (ObjectKind::Environment, *id)),
                )
                .await?,
            ];
            let known = native::bind_known(ctx, &RECOVER_PREFLIGHT.declaration, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            let mut managed = Vec::new();
            for id in existing {
                match orch_engine::operation_access::review(
                    conn,
                    &principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id,
                    },
                    Permission::EnvironmentManage,
                )
                .await
                {
                    Ok(_) => managed.push(id),
                    Err(UpdateError::Denied | UpdateError::NotFound) => {}
                    Err(e) => return Err(super::super::updates::refused(e)),
                }
            }
            let selected = vec![
                common::objects(
                    conn,
                    Source::Environments,
                    managed.iter().map(|id| (ObjectKind::Environment, *id)),
                )
                .await?,
            ];
            Ok((Input { body, managed }, selected))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        p: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            for id in &input.managed {
                orch_engine::operation_access::review(
                    conn,
                    p,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: *id,
                    },
                    Permission::EnvironmentManage,
                )
                .await
                .map_err(super::super::updates::refused)?;
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
            let scope = common::source(
                ctx,
                &mut tx,
                caller,
                Kind::RecoverPreflight,
                &serde_json::to_value(&input.body).map_err(ApiError::internal)?,
                &input.managed,
                None,
            )
            .await?;
            tx.commit().await.map_err(DbError::from)?;
            let current = if input.managed.is_empty() {
                caller.clone()
            } else {
                scope
                    .current(&ctx.app.engine)
                    .await
                    .map_err(common::error)?
            };
            preparation::scope(
                &scope,
                super::observe_preflight(ctx.clone(), &current, &scope, input.body),
            )
            .await
        })
    }
}

impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = api::EnvironmentRecoverPreflightBody;
    type Value = Arc<Observed>;
    type Output = Arc<Observed>;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &RECOVER_PREFLIGHT.declaration
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        _: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = effects::initial_value::<Self>(ctx)?;
            if value.plan.expires_at <= ctx.app.clock.time() {
                return Err(super::super::updates::refused(UpdateError::Expired));
            }
            common::current(ctx, conn, &value.origin).await?;
            let facts = value
                .origin
                .facts(conn, true)
                .await
                .map_err(common::source_error)?;
            Ok((value, facts))
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
            let stored = serde_json::to_value(&value.stored).map_err(ApiError::internal)?;
            let deadline = before.deadline();
            before.check()?;
            jobs::forks::save_preflight(
                conn,
                &jobs::forks::PreflightRow {
                    id: value.plan.preflight_id,
                    kind: "recover_batch".into(),
                    node_id: None,
                    environment_id: None,
                    created_by: p.user(),
                    plan: stored,
                    expires_at: value.plan.expires_at.get(),
                    created_at: ctx.now,
                },
            )
            .await?;
            orch_db::queries::notifications::operations::preview(
                conn,
                value.plan.preflight_id,
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
    type Reply = api::EnvironmentRecoveryPlan;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Self::Input) -> &'static Declaration<Source> {
        &RECOVER_PREFLIGHT.declaration
    }
    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Self::Input,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let saved = jobs::forks::preflight(conn, input.plan.preflight_id)
                .await?
                .filter(|r| {
                    r.kind == "recover_batch" && r.created_by == input.origin.captured().user()
                })
                .ok_or_else(ApiError::not_found)?;
            if saved.plan != serde_json::to_value(&input.stored).map_err(ApiError::internal)?
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
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Self::Input,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::EnvironmentRecoveryPlan>> {
        Box::pin(async move { Ok(input.plan.clone()) })
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
    _: api::EnvironmentRecoverPreflightBody,
) -> ApiResult<api::EnvironmentRecoverPreflightResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::EnvironmentRecoverPreflightReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: done.output.plan.preflight_id,
        expires_at: done.output.plan.expires_at,
        executable: done.output.plan.executable,
    };
    publication::receipt(&done.ctx, &receipt)?;
    Ok(
        match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await? {
            Some(full) => api::EnvironmentRecoverPreflightResponse::full(full),
            None => api::EnvironmentRecoverPreflightResponse::Receipt(receipt),
        },
    )
}
