//! A thread's goal: read, set, edited, paused and cleared by whoever drives the thread.
//!
//! What a goal needs of its caller is asked of the access engine inside the engine's transaction, where the
//! thread's row is locked, and told whole in one refusal; so these requests are declared with a rule and decided
//! there. The goal itself rides on the thread, so the thread's readers see every change as it is made.

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::goals::ThreadGoal;

use orch_api_contract::goals::ThreadGoalClear;

use orch_api_contract::goals::ThreadGoalCreate;

use orch_api_contract::goals::ThreadGoalCreateBody;

use orch_api_contract::goals::ThreadGoalCreateResponse;

use orch_api_contract::goals::ThreadGoalGet;

use orch_api_contract::goals::ThreadGoalUpdate;

use orch_api_contract::goals::ThreadGoalUpdateBody;

use orch_engine::goals::GoalError;

use orch_engine::inputs::InputError;

use orch_types::Id;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(ThreadGoalCreate, operation::replay_binding())
        .on_with_operation(ThreadGoalGet, crate::key_operations::direct::declared, get)
        .on_with_operation(
            ThreadGoalCreate,
            crate::key_operations::native::effects::prepare::<operation::Create>,
            create,
        )
        .on(ThreadGoalUpdate, update)
        .on(ThreadGoalClear, clear);
}

/// Why a goal could not be set, changed or cleared, told as the API tells it.
async fn goal_failed(ctx: &Ctx, thread: Id, error: GoalError) -> ApiError {
    match ctx.app.db.acquire().await {
        Ok(mut conn) => goal_failed_in(ctx, &mut conn, thread, error).await,
        Err(error) => error.into(),
    }
}

/// Preparation already owns its connection; named errors never acquire another pool permit.
async fn goal_failed_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    thread: Id,
    error: GoalError,
) -> ApiError {
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
    let restricted = ctx.key_receipt_required()
        || ctx.user().is_ok_and(|user| {
            matches!(
                &user.via,
                crate::middleware::authentication::Via::Token {
                    operation_scope: Some(_),
                    ..
                }
            )
        });
    super::input_failed_projected(conn, thread, shared, restricted).await
}

/// The thread's goal as it stands.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<Option<ThreadGoal>> {
    let thread = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    authorization::require_on_path_in(&ctx, &mut conn, &principal, "thread.view").await?;
    orch_engine::goals::view(&mut conn, thread, principal.now().get())
        .await
        .map_err(ApiError::engine)
}

/// Sets a goal on the thread in the caller's name.
async fn create(
    ctx: Ctx,
    (): (),
    _body: ThreadGoalCreateBody,
) -> ApiResult<ThreadGoalCreateResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed =
        crate::key_operations::native::effects::apply_owned::<operation::Create>(&ctx, tx).await?;
    operation::response(completed).await
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

#[path = "goals/operation.rs"]
pub mod operation;
