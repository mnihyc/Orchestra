//! Current native commands and guarded acceptance of exact reviewed End jobs.

use crate::middleware::authentication::Via;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Answer, Ctx, Registry};
use orch_access_contract::decisions::Decision;
use orch_api_contract::background::{
    BackgroundEndRequest, ThreadBackground, ThreadBackgroundEnd, ThreadBackgroundGet,
    ThreadBackgroundGetQuery,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::workspace::Job;
use orch_engine::background::{BackgroundError, Identity};

/// Binds the roster read and the captured mutation request.
pub fn mount(registry: &mut Registry) {
    registry
        .on(ThreadBackgroundGet, get)
        .on(ThreadBackgroundEnd, end);
}

fn identity(via: &Via) -> Identity {
    match via {
        Via::Session { session_id } => Identity {
            credential_id: *session_id,
            token: false,
        },
        Via::Token { token_id, .. } => Identity {
            credential_id: *token_id,
            token: true,
        },
    }
}

async fn failed(ctx: &Ctx, error: BackgroundError) -> ApiError {
    match error {
        BackgroundError::NotFound => ApiError::not_found(),
        BackgroundError::Stale => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the reviewed command list expired or changed; refresh it before End",
        )
        .with("refresh", "background"),
        BackgroundError::Invalid(message) => ApiError::invalid("", message),
        BackgroundError::Denied(decision) => match *decision {
            Decision::Refused(refusal) => match ctx.app.db.acquire().await {
                Ok(mut conn) => match refusals::denied(&mut conn, refusal).await {
                    Ok(error) | Err(error) => error,
                },
                Err(error) => ApiError::internal(error),
            },
            Decision::Allowed { .. } => {
                ApiError::internal("an allowed background operation was refused")
            }
        },
        BackgroundError::Internal(error) => ApiError::engine(error),
    }
}

async fn get(ctx: Ctx, query: ThreadBackgroundGetQuery, (): ()) -> ApiResult<ThreadBackground> {
    let user = ctx.user()?;
    let caller = authorization::caller_of(user)?;
    match ctx
        .app
        .engine
        .background(
            &caller,
            identity(&user.via),
            ctx.id("id")?,
            query.cursor.as_deref(),
            query.limit,
        )
        .await
    {
        Ok(roster) => Ok(roster),
        Err(error) => Err(failed(&ctx, error).await),
    }
}

async fn end(ctx: Ctx, (): (), request: BackgroundEndRequest) -> ApiResult<Answer<Job>> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let identity = identity(&guard.user().via);
    let row = match ctx
        .app
        .engine
        .end_background_guarded(tx, guard.into_principal(), identity, thread, request)
        .await
    {
        Ok(row) => row,
        Err(error) => return Err(failed(&ctx, error).await),
    };
    let mut conn = ctx.app.db.acquire().await?;
    let job = crate::routes::environments::jobs::view(&mut conn, &row).await?;
    Ok(Answer::from(job))
}
