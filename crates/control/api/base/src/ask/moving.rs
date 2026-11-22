//! A built-in conversation uses its real copier and the original native Delete in one transaction.

use super::*;
use crate::registry::{OperationFuture, OperationPlan};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ResourceKind;
use orch_authz::Policy;
use orch_db::queries::jobs::forks::{self, PreflightRow};
use orch_engine::fork_plan::{
    moving,
    operation::{MoveConfirm, MovePreview},
};
use serde::{Deserialize, Serialize};

const KIND: &str = "ask_move_out";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Saved {
    copy: api::AskKeepBody,
    review: moving::Review,
}

#[derive(Debug, Clone)]
struct Selected {
    saved: Saved,
    destination: keep::Destination,
}

fn stale() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "The reviewed conversation or its original deletion changed",
    )
}

async fn saved(ctx: &Ctx, conn: &mut Conn, person: Id, id: Id) -> ApiResult<Saved> {
    let plan = forks::preflight(conn, id)
        .await?
        .filter(|plan| {
            plan.kind == KIND
                && plan.node_id == ctx.id("id").ok()
                && plan.created_by == person
                && plan.expires_at > ctx.now
        })
        .ok_or_else(stale)?;
    serde_json::from_value(plan.plan).map_err(ApiError::internal)
}

async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &orch_authz::Caller,
    body: api::AskKeepBody,
) -> ApiResult<OperationPlan> {
    if body.topic_id.is_none() || body.project_id.is_some() || body.to_turn.is_some() {
        return Err(ApiError::invalid(
            "topic_id",
            "Choose a destination topic for the whole conversation",
        ));
    }
    let id = ctx.id("id")?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let source = crate::key_operations::targets::node(conn, id).await?;
    for facet in [0, 1] {
        plan.bind(
            facet,
            KeyTargetKind::Object(ObjectKind::Thread),
            std::slice::from_ref(&source),
        )?;
    }
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let row = keep::source(ctx, conn, &principal, id, false).await?;
    if row.temporary {
        return Err(ApiError::invalid(
            "source",
            "Keep the temporary conversation before moving it",
        ));
    }
    if !orch_engine::copy_out::owns(
        conn,
        &principal,
        orch_engine::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
            kind: ObjectKind::Thread,
            id,
        }),
    )
    .await
    .map_err(ApiError::internal)?
    {
        return Err(denied("Only an actual owner may move this conversation"));
    }
    let destination = keep::destination(ctx, conn, &principal, &row, &body).await?;
    let review = moving::capture(conn, id, destination.node)
        .await
        .map_err(ApiError::engine)?;
    let target = crate::key_operations::targets::node(conn, destination.node).await?;
    plan.bind(2, KeyTargetKind::Object(ObjectKind::Thread), &[target])?;
    let resources = moving::resources(conn, &review)
        .await
        .map_err(ApiError::engine)?;
    if resources.is_empty() {
        plan.bind_empty(3)?;
    } else {
        plan.bind_collection(3, &resources)?;
    }
    Ok(plan.with_data(Selected {
        saved: Saved { copy: body, review },
        destination,
    }))
}

pub(super) fn prepare_preview<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    body: api::AskKeepBody,
) -> OperationFuture<'a> {
    Box::pin(async move { select(ctx, conn, caller, body).await })
}

pub(super) fn prepare_move<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    body: api::AskMoveOutBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let old = saved(ctx, conn, caller.user(), body.preflight_id).await?;
        let plan = select(ctx, conn, caller, old.copy).await?;
        // The handler repeats this comparison under its transaction and the original Delete locks.
        Ok(plan)
    })
}

async fn require_resources(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<()> {
    let native = orch_engine::archive::deletion::inspect(conn, id)
        .await
        .map_err(ApiError::internal)?;
    for resource in native.resources() {
        let kind: ResourceKind = resource.kind.parse().map_err(ApiError::internal)?;
        let permission = match kind {
            ResourceKind::Provider => Permission::ProviderManage,
            ResourceKind::Agent => Permission::AgentManage,
            ResourceKind::Environment => Permission::EnvironmentManage,
            ResourceKind::Host => Permission::HostManage,
            ResourceKind::Machine => Permission::MachineManage,
            ResourceKind::Template => Permission::TemplateManage,
            ResourceKind::McpServer => Permission::McpManage,
            ResourceKind::Mount => Permission::MountManage,
            ResourceKind::Skill => Permission::SkillManage,
            ResourceKind::Pool => Permission::PoolManage,
        };
        authorization::require_resource(
            conn,
            principal,
            kind,
            resource.id,
            permission,
            Policy::default(),
        )
        .await?;
    }
    Ok(())
}

pub(super) async fn preview(
    ctx: Ctx,
    (): (),
    _body: api::AskKeepBody,
) -> ApiResult<api::AskMoveOutPlan> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let selected = guard.operation_data::<Selected>()?;
    moving::require::<MovePreview>(&mut tx, guard.principal(), &selected.saved.review)
        .await
        .map_err(ApiError::engine)?;
    require_resources(&mut tx, guard.principal(), ctx.id("id")?).await?;
    let sides = threads::forking::side_threads_of(&mut tx, ctx.id("id")?, false).await?;
    let refs =
        crate::routes::nodes::view::refs(&mut tx, guard.principal(), &sides, Policy::default())
            .await?;
    let id = new_id();
    guard.check_deadline(&ctx)?;
    forks::save_preflight(
        &mut tx,
        &PreflightRow {
            id,
            kind: KIND.into(),
            node_id: Some(ctx.id("id")?),
            environment_id: None,
            created_by: guard.user().id(),
            plan: serde_json::to_value(&selected.saved).map_err(ApiError::internal)?,
            expires_at: ctx.now
                + chrono::Duration::minutes(orch_api_contract::forks::PREFLIGHT_VALID_MINUTES),
            created_at: ctx.now,
        },
    )
    .await?;
    let reply = api::AskMoveOutPlan {
        preflight_id: id,
        delete_plan: selected.saved.review.public(),
        open_side_threads: sides
            .iter()
            .filter_map(|id| refs.get(id).cloned())
            .collect(),
    };
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(reply)
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    body: api::AskMoveOutBody,
) -> ApiResult<api::AskKeepReply> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let old = saved(&ctx, &mut tx, guard.user().id(), body.preflight_id).await?;
    let selected = guard.operation_data::<Selected>()?;
    if old.review != selected.saved.review {
        return Err(stale());
    }
    require_resources(&mut tx, guard.principal(), ctx.id("id")?).await?;
    moving::require::<MoveConfirm>(&mut tx, guard.principal(), &old.review)
        .await
        .map_err(ApiError::engine)?;
    let row = keep::source(&ctx, &mut tx, guard.principal(), ctx.id("id")?, true).await?;
    let ending = moving::prepare_ending(&ctx.app.engine, &mut tx, guard.principal(), &old.review)
        .await
        .map_err(ApiError::engine)?;
    guard.check_deadline(&ctx)?;
    let copied = keep::copy_in(
        &ctx,
        &mut tx,
        guard.principal(),
        guard.user().id(),
        keep::Copy {
            row: &row,
            body: &old.copy,
            destination: selected.destination,
            private: false,
        },
    )
    .await?;
    guard.check_deadline(&ctx)?;
    moving::finish(
        &ctx.app.engine,
        &mut tx,
        &ending,
        guard.user().id(),
        Time::new(ctx.now),
    )
    .await
    .map_err(ApiError::engine)?;
    creation::observe_move(
        &mut tx,
        guard.user().id(),
        row.id,
        copied.thread_id,
        copied.topic_id,
    )
    .await?;
    forks::forget_preflight(&mut tx, body.preflight_id).await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app.engine.kick();
    crate::key_operations::publication::receipt(&ctx, &copied)?;
    Ok(copied)
}
