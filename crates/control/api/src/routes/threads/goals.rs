//! A thread's goal: read, set, edited, paused and cleared by whoever drives the thread.
//!
//! What a goal needs of its caller is asked of the access engine inside the engine's transaction, where the
//! thread's row is locked, and told whole in one refusal; so these requests are declared with a rule and decided
//! there. The goal itself rides on the thread, so the thread's readers see every change as it is made.

use super::input_failed;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::goals::{
    ThreadGoal, ThreadGoalClear, ThreadGoalCreate, ThreadGoalCreateBody, ThreadGoalGet,
    ThreadGoalUpdate, ThreadGoalUpdateBody,
};
use orch_engine::goals::GoalError;
use orch_engine::inputs::InputError;
use orch_types::Id;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ThreadGoalGet, get)
        .on(ThreadGoalCreate, create)
        .on(ThreadGoalUpdate, update)
        .on(ThreadGoalClear, clear);
}

/// Why a goal could not be set, changed or cleared, told as the API tells it.
async fn goal_failed(ctx: &Ctx, thread: Id, error: GoalError) -> ApiError {
    let shared = match error {
        GoalError::Exists => {
            return ApiError::new(
                ErrorCode::GoalExists,
                "a goal is under way on this thread already",
            );
        }
        GoalError::LimitReached => {
            return ApiError::new(
                ErrorCode::GoalLimitReached,
                "a limit of the goal is reached: raise it first",
            );
        }
        GoalError::NotFound => InputError::NotFound,
        GoalError::Denied(refusal) => InputError::Denied(refusal),
        GoalError::Archived => InputError::Archived,
        GoalError::Invalid(words) => InputError::Invalid(words),
        GoalError::Internal(error) => InputError::Internal(error),
    };
    match ctx.app.db.acquire().await {
        Ok(mut conn) => input_failed(&mut conn, thread, shared).await,
        Err(error) => error.into(),
    }
}

/// The thread's goal as it stands.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<ThreadGoal>> {
    let thread = ctx.id("id")?;
    ctx.app
        .engine
        .goal_of(thread)
        .await
        .map_err(ApiError::engine)
}

/// Sets a goal on the thread in the caller's name.
async fn create(ctx: Ctx, (): (), body: ThreadGoalCreateBody) -> ApiResult<ThreadGoal> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .set_goal_guarded(tx, guard.into_principal(), thread, &body)
        .await
    {
        Ok(goal) => Ok(goal),
        Err(error) => Err(goal_failed(&ctx, thread, error).await),
    }
}

/// Edits the goal, pauses it, or sets it going again.
async fn update(ctx: Ctx, (): (), body: ThreadGoalUpdateBody) -> ApiResult<ThreadGoal> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .update_goal_guarded(tx, guard.into_principal(), thread, &body)
        .await
    {
        Ok(goal) => Ok(goal),
        Err(error) => Err(goal_failed(&ctx, thread, error).await),
    }
}

/// Takes the goal away.
async fn clear(ctx: Ctx, (): (), (): ()) -> ApiResult<ThreadGoal> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    match ctx
        .app
        .engine
        .clear_goal_guarded(tx, guard.into_principal(), thread)
        .await
    {
        Ok(goal) => Ok(goal),
        Err(error) => Err(goal_failed(&ctx, thread, error).await),
    }
}
