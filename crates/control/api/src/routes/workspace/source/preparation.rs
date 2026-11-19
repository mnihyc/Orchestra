//! The existing source planner supplies one immutable native job; the new acknowledgement is generated.

use super::*;
use crate::key_operations::native::{
    self,
    effects::{self, Apply, Effect, Outcome},
    reply,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{operation_sources::Declaration, source_sources::Source as Role};
use orch_db::queries::environments::workspace::EnvironmentRow;
use orch_engine::jobs::source::operation;

pub(super) struct Prepare;
pub(super) struct Value {
    plan: Plan,
    env: EnvironmentRow,
    facts: Vec<native::Facts<Role>>,
}
pub(super) struct Output {
    row: orch_db::queries::jobs::JobRow,
}
// Actual same-request child association, never source admission or a skipped ordinary gate.
#[derive(PartialEq)]
struct Created(orch_db::queries::jobs::JobRow);
async fn native_mode(conn: &mut Conn, body: &EnvironmentSourcePrepareBody) -> ApiResult<bool> {
    let row = threads::thread(conn, body.thread_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if body.source.is_none()
        && let Some(value) = row
            .source
            .as_ref()
            .and_then(|value| value.get("native_plan"))
    {
        let plan: Plan = serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
        return Ok(matches!(plan.action, Action::Worktree { .. }));
    }
    Ok(row
        .source
        .as_ref()
        .and_then(|value| value.get("worktree"))
        .is_some_and(|value| !value.is_null()))
}
async fn actor(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    body: &EnvironmentSourcePrepareBody,
) -> ApiResult<Principal> {
    let mode = native_mode(conn, body).await?;
    let facts = operation::facts_for(conn, ctx.id("id")?, body.thread_id, mode)
        .await
        .map_err(ApiError::engine)?;
    let plan = native::bind_required(ctx, operation::declaration_for(mode), &facts)?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    require(
        conn,
        &principal,
        ctx.id("id")?,
        Permission::EnvironmentManage,
    )
    .await?;
    authorization::require(
        conn,
        &principal,
        body.thread_id,
        authorization::VIEW,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await?;
    Ok(principal)
}
impl Effect for Prepare {
    type Source = Role;
    type Query = ();
    type Body = EnvironmentSourcePrepareBody;
    type Value = Value;
    type Output = Output;
    fn declaration(value: &Value) -> &'static Declaration<Role> {
        operation::declaration(&value.plan)
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        body: EnvironmentSourcePrepareBody,
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let preliminary = actor(ctx, conn, caller, &body).await?;
            orch_db::queries::environments::hosting::lock_environment(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if let Some(open) =
                orch_db::queries::jobs::retained::open_of_environment(conn, ctx.id("id")?)
                    .await?
                    .into_iter()
                    .next()
            {
                let created = ctx.optional_retained_source::<Created>()?;
                if !created.as_ref().is_some_and(|created| {
                    created.0.id == open.id
                        && created.0.environment_id == open.environment_id
                        && created.0.node_id == open.node_id
                        && created.0.created_by == open.created_by
                }) {
                    // Keep the native Env-before-Thread creator order without waiting on an
                    // active source job's Thread while holding the Env it needs to finish.
                    let original = make_error(orch_engine::jobs::runner::MakeError::Running(
                        Box::new(open),
                    ));
                    let sparse = authorization::problem_without_content(original.clone());
                    let worktree = native_mode(conn, &body).await?;
                    let facts = operation::facts_for(conn, ctx.id("id")?, body.thread_id, worktree)
                        .await
                        .map_err(ApiError::engine)?;
                    return Err(native::refusal::project::<FailureProjection>(
                        ctx,
                        preliminary.caller(),
                        Failed {
                            env: ctx.id("id")?,
                            original,
                            facts,
                            worktree,
                        },
                        sparse,
                    )
                    .await?);
                }
            }
            threads::lock(conn, body.thread_id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR KEY SHARE")
                .bind(body.thread_id)
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            let identity = transaction_guard::current_http_identity(
                ctx,
                conn,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            let caller = identity
                .caller
                .intersect_restrictions(caller)
                .map_err(ApiError::internal)?;
            let principal = actor(ctx, conn, &caller, &body).await?;
            let (plan, env) = prepare_native(ctx, conn, principal, body).await?;
            let facts = operation::facts(conn, &plan)
                .await
                .map_err(ApiError::engine)?;
            Ok((
                Value {
                    plan,
                    env,
                    facts: facts.clone(),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require(
                conn,
                principal,
                value.plan.environment,
                Permission::EnvironmentManage,
            )
            .await?;
            authorization::require(
                conn,
                principal,
                value.plan.thread.ok_or_else(ApiError::not_found)?,
                authorization::VIEW,
                orch_authz::Act::Use,
                Policy::default(),
            )
            .await
        })
    }
}
impl Apply for Prepare {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Output>>> {
        Box::pin(async move {
            let row = match source::begin_before(conn, value.plan.clone(), ctx.now, move || {
                before
                    .check()
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            })
            .await
            {
                Ok(row) => row,
                Err(failure) => {
                    let original = make_error(failure);
                    let sparse = authorization::problem_without_content(original.clone());
                    let input = Failed {
                        env: value.env.id,
                        original,
                        facts: value.facts.clone(),
                        worktree: matches!(value.plan.action, Action::Worktree { .. }),
                    };
                    return Err(native::refusal::project::<FailureProjection>(
                        ctx,
                        principal.caller(),
                        input,
                        sparse,
                    )
                    .await?);
                }
            };
            let session = match ctx.user()?.via {
                crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
                crate::middleware::authentication::Via::Token { .. } => None,
            };
            operation::Request::retain(
                conn,
                &row,
                principal.caller(),
                session,
                &value.env.workspace_root,
                orch_types::Time::new(ctx.now),
            )
            .await
            .map_err(ApiError::engine)?;
            ctx.retain_source(Created(row.clone()))?;
            Ok(Outcome::Applied(Output { row }))
        })
    }
}
struct Failed {
    env: Id,
    original: ApiError,
    facts: Vec<native::Facts<Role>>,
    worktree: bool,
}
struct FailureProjection;
impl reply::Projection for FailureProjection {
    type Source = Role;
    type Input = Failed;
    type Subject = EnvironmentRow;
    type Value = ();
    type Reply = ApiError;
    const READ_SOURCE: Role = Role::ProblemEnvironment;
    fn declaration(input: &Failed) -> &'static Declaration<Role> {
        operation::declaration_for(input.worktree)
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Failed,
    ) -> BoxFuture<'a, ApiResult<(EnvironmentRow, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let env = orch_db::queries::environments::workspace::environment(conn, input.env)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let target = crate::key_operations::targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: env.id,
                },
            )
            .await?;
            let mut facts = input
                .facts
                .iter()
                .filter(|fact| fact.source != Role::ProblemEnvironment)
                .cloned()
                .collect::<Vec<_>>();
            facts.push(native::Facts::one(
                Role::ProblemEnvironment,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            ));
            Ok((env, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Failed,
        env: &'a EnvironmentRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match require(conn, principal, env.id, Permission::FilesView).await {
                Ok(()) => Ok(()),
                Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
                    require(conn, principal, env.id, Permission::EnvironmentManage).await
                }
                Err(error) => Err(error),
            }
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Failed,
        _: &'a EnvironmentRow,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Role>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Failed,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move { Ok(input.original.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Failed,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.generation == fresh.subject.generation
                && old.subject.machine_id == fresh.subject.machine_id
                && old.subject.workspace_root == fresh.subject.workspace_root)
        })
    }
}
pub(super) async fn run(ctx: Ctx, _: (), _: EnvironmentSourcePrepareBody) -> ApiResult<Job> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<Prepare>(&ctx, &mut tx).await?;
    let (current, guard) = transaction_guard::after_wait(&completed.ctx, &mut tx).await?;
    guard.check_deadline(&current)?;
    // This immutable just-created body has only supplied references/generated state, no old
    // result/log/error or source configuration. It is the native generated acknowledgement.
    let shown = crate::routes::environments::jobs::view(&mut tx, &completed.output.row).await?;
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    crate::key_operations::publication::receipt(&current, &shown)?;
    current.app.engine.kick();
    Ok(shown)
}
