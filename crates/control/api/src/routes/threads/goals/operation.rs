//! A goal uses its native prepared writer and the shared typed completed projection.

use super::{goal_failed, goal_failed_in};
use crate::key_operations::native::{effects, reply};
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::goals::{
    ThreadGoal, ThreadGoalCreateBody, ThreadGoalCreateResponse, ThreadGoalReceipt,
};
use orch_api_contract::operation_sources::Declaration;
use orch_api_contract::thread_sources::{self as declarations, Source};
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_db::queries::{goals, threads};
use orch_engine::goals::{CreatedGoal, PreparedGoal};
use orch_types::Id;

const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

async fn facts(conn: &mut Conn, thread: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::node(conn, thread).await?;
    if target.object.kind != THREAD {
        return Err(ApiError::not_found());
    }
    Ok(vec![native::Facts::one(Source::Thread, THREAD, target)])
}

pub(super) struct Create;

impl effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = ThreadGoalCreateBody;
    type Value = PreparedGoal;
    type Output = CreatedGoal;

    fn declaration(_value: &PreparedGoal) -> &'static Declaration<Source> {
        &declarations::GOAL_CREATE
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: ThreadGoalCreateBody,
    ) -> BoxFuture<'a, ApiResult<(PreparedGoal, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let thread = ctx.id("id")?;
            if native::modern(caller) {
                let candidate = targets::node(conn, thread).await?;
                if candidate.object.kind != THREAD {
                    return Err(ApiError::not_found());
                }
                let declared = crate::registry::OperationPlan::new(
                    ctx.route,
                    declarations::GOAL_CREATE.operation,
                )?;
                if !native::permits(
                    &declared,
                    &declarations::GOAL_CREATE,
                    caller,
                    &Source::Thread,
                    THREAD,
                    &candidate,
                )? {
                    return Err(ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the key does not allow this operation",
                    ));
                }
            }
            threads::lock(conn, thread)
                .await?
                .filter(|row| row.deleted_at.is_none())
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, thread).await?;
            let base = native::bind_known(ctx, &declarations::GOAL_CREATE, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &base).await?;
            let value = ctx
                .app
                .engine
                .prepare_goal(conn, &principal, thread, &body)
                .await;
            match value {
                Ok(value) => Ok((value, facts)),
                Err(error) => Err(goal_failed_in(ctx, conn, thread, error).await),
            }
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a PreparedGoal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .require_goal_preparation(conn, principal, value)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(goal_failed_in(ctx, conn, value.thread(), error).await),
            }
        })
    }
}

impl effects::ApplyOwned for Create {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a PreparedGoal,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<CreatedGoal>>> {
        Box::pin(async move {
            let result = ctx
                .app
                .engine
                .create_prepared_goal_guarded(tx, principal.clone(), value, move || {
                    before.check().map_err(|error| {
                        orch_engine::error::EngineError::BeforeEffect(Box::new(error))
                    })
                })
                .await;
            match result {
                Ok(created) => Ok(effects::Outcome::Applied(created)),
                Err(error) => Err(goal_failed(ctx, value.thread(), error).await),
            }
        })
    }
}

pub(super) async fn response(
    completed: effects::Completed<CreatedGoal>,
) -> ApiResult<ThreadGoalCreateResponse> {
    let receipt = ThreadGoalReceipt {
        projection: ReceiptProjection::Receipt,
        thread_id: completed.output.thread(),
        goal_id: completed.output.id(),
    };
    let fallback = ThreadGoalCreateResponse::Receipt(receipt);
    crate::key_operations::publication::receipt(&completed.ctx, &fallback)?;
    match reply::render::<Readback>(&completed.ctx, &completed.accepted, completed.output).await? {
        Some(value) => Ok(ThreadGoalCreateResponse::Full(Box::new(value))),
        None => Ok(fallback),
    }
}

struct Readback;
pub(super) struct Subject {
    thread: Id,
    goal: Id,
}

impl reply::Projection for Readback {
    type Source = Source;
    type Input = CreatedGoal;
    type Subject = Subject;
    type Value = goals::GoalRow;
    type Reply = ThreadGoal;
    const READ_SOURCE: Source = Source::Thread;

    fn declaration(_input: &CreatedGoal) -> &'static Declaration<Source> {
        &declarations::GOAL_CREATE
    }

    fn subject<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a CreatedGoal,
    ) -> BoxFuture<'a, ApiResult<(Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let current = goals::current(conn, input.thread())
                .await?
                .filter(|row| row.id == input.id())
                .ok_or_else(ApiError::not_found)?;
            let subject = Subject {
                thread: current.thread_id,
                goal: current.id,
            };
            Ok((subject, facts(conn, current.thread_id).await?))
        })
    }

    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a CreatedGoal,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match ctx
                .app
                .engine
                .check_goal_readback(conn, principal, subject.thread)
                .await
            {
                Ok(()) => Ok(()),
                Err(error) => Err(goal_failed_in(ctx, conn, subject.thread, error).await),
            }
        })
    }

    fn prepare<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a CreatedGoal,
        subject: &'a Subject,
    ) -> BoxFuture<'a, ApiResult<(goals::GoalRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = goals::current(conn, subject.thread)
                .await?
                .filter(|row| row.id == subject.goal)
                .ok_or_else(ApiError::not_found)?;
            Ok((row, Vec::new()))
        })
    }

    fn render<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a CreatedGoal,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ThreadGoal>> {
        Box::pin(async move {
            orch_engine::goals::view(conn, prepared.subject.thread, principal.now().get())
                .await
                .map_err(ApiError::engine)?
                .filter(|goal| goal.id == prepared.value.id)
                .ok_or_else(ApiError::not_found)
        })
    }

    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a CreatedGoal,
        opened: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.subject.thread == fresh.subject.thread
                && opened.subject.goal == fresh.subject.goal
                && opened.value.id == fresh.value.id
                && opened.value.thread_id == fresh.value.thread_id)
        })
    }
}
