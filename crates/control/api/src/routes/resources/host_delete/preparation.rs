//! Once-only native preview construction and guarded persistence of its actual source facts.

use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication,
};
use futures::future::BoxFuture;
use orch_api_contract::{
    host_delete_sources::{PREFLIGHT, Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use std::sync::Arc;

pub(super) struct Preview;
pub(super) struct Input {
    host: Id,
    id: Id,
    body: api::HostDeletePreflightBody,
}
pub(super) struct Ready {
    native: Arc<Stored>,
}
pub(super) struct Saved {
    native: Arc<Stored>,
}
impl effects::Effect for Preview {
    type Source = Source;
    type Query = ();
    type Body = api::HostDeletePreflightBody;
    type Value = Ready;
    type Output = Saved;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        _: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let native = effects::initial_value::<Self>(ctx)?;
            if ctx.id("id")? != native.host {
                return Err(ApiError::not_found());
            }
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                orch_db::queries::resources::lock_resource(conn, "host", native.host).await?;
                for action in &native.environments {
                    let id = match action {
                        EnvironmentAction::Recreate { recipe } => recipe.environment,
                        EnvironmentAction::Move { recipe, .. } => recipe.replacement.target.id(),
                        EnvironmentAction::Portable { recipe, .. } => recipe.environment,
                        EnvironmentAction::Remove { id, .. } => *id,
                    };
                    envs::lock_environment(conn, id).await?;
                }
            }
            let current = loss::inventory(conn, "host", native.host)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if current.fingerprint()? != native.fingerprint {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "The host's dependencies changed; review its deletion again",
                ));
            }
            if native.plan.expires_at <= ctx.app.clock.time() {
                return Err(refused(UpdateError::Expired));
            }
            let facts = source::required(conn, &native).await?;
            Ok((Ready { native }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { source::require(conn, principal, &value.native).await })
    }
}
impl effects::Initial for Preview {
    type Input = Input;
    type Fetched = Stored;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let host = ctx.id("id")?;
            Ok((
                Input {
                    host,
                    id: new_id(),
                    body,
                },
                vec![source::host_fact(conn, host).await?],
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
            operation_access::review(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: input.host,
                },
                Permission::HostManage,
            )
            .await
            .map_err(refused)?;
            Ok(())
        })
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        caller: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Stored>> {
        Box::pin(async move {
            let principal = {
                let mut conn = ctx.app.db.acquire().await?;
                let facts = vec![source::host_fact(&mut conn, input.host).await?];
                let known = native::bind_known(ctx, &PREFLIGHT, &facts)?;
                native::preparation_principal(ctx, &mut conn, caller, &known).await?
            };
            build(ctx, &principal, input.host, input.id, &input.body).await
        })
    }
}
impl effects::Apply for Preview {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Saved>>> {
        Box::pin(async move {
            let native = &value.native;
            let deadline = before.deadline();
            before.check()?;
            save(ctx, conn, principal, native).await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Saved {
                native: native.clone(),
            }))
        })
    }
}
pub(super) async fn save(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    stored: &Stored,
) -> ApiResult<()> {
    jobs::forks::save_preflight(
        conn,
        &jobs::forks::PreflightRow {
            id: stored.plan.preflight_id,
            kind: "host_delete".into(),
            node_id: None,
            environment_id: None,
            created_by: principal.user(),
            plan: serde_json::to_value(stored).map_err(ApiError::internal)?,
            expires_at: stored.plan.expires_at.get(),
            created_at: ctx.now,
        },
    )
    .await?;
    orch_db::queries::notifications::operations::preview(
        conn,
        stored.plan.preflight_id,
        &stored
            .audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?;
    Ok(())
}

struct Content;
struct Subject;
impl reply::Projection for Content {
    type Source = Source;
    type Input = Saved;
    type Subject = Subject;
    type Value = ();
    type Reply = api::HostDeletePlan;
    const READ_SOURCE: Source = Source::Content;
    fn declaration(_: &Saved) -> &'static Declaration<Source> {
        &PREFLIGHT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Saved,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.native.plan.preflight_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.kind != "host_delete" {
                return Err(ApiError::not_found());
            }
            let mut facts = source::required(conn, &input.native).await?;
            facts.push(source::content(conn, &input.native).await?);
            Ok((Subject, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Saved,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            source::require(conn, principal, &input.native).await?;
            // These are the actual privileged native recovery rows, not every Host dependency.
            for id in &input.native.read_environments {
                operation_access::review(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: *id,
                    },
                    Permission::EnvironmentManage,
                )
                .await
                .map_err(refused)?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        _: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<api::HostDeletePlan>> {
        Box::pin(async move { Ok(input.native.plan.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        _: &'a reply::Prepared<Self>,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let row = jobs::forks::preflight(conn, input.native.plan.preflight_id).await?;
            Ok(row.is_some_and(|row| {
                row.kind == "host_delete" && row.plan.get("host") == Some(&json!(input.native.host))
            }))
        })
    }
}
pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::HostDeletePreflightBody,
) -> ApiResult<api::HostDeletePreflightResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Preview>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::HostDeletePreflightReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        host_id: done.output.native.host,
        preflight_id: done.output.native.plan.preflight_id,
        expires_at: done.output.native.plan.expires_at,
        executable: done.output.native.plan.executable,
    };
    publication::receipt(&done.ctx, &receipt)?;
    match reply::render::<Content>(&done.ctx, &done.accepted, done.output).await? {
        Some(full) => Ok(api::HostDeletePreflightResponse::Full(Box::new(full))),
        None => Ok(api::HostDeletePreflightResponse::Receipt(receipt)),
    }
}
