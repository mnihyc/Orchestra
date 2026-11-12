//! Current native commands and guarded acceptance of exact reviewed End jobs.

use crate::middleware::authentication::Via;

use crate::middleware::authorization;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Answer;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::Registry;

use orch_access_contract::decisions::Decision;

use orch_api_contract::background::BackgroundEndRequest;

use orch_api_contract::background::ThreadBackground;

use orch_api_contract::background::ThreadBackgroundEnd;

use orch_api_contract::background::ThreadBackgroundGet;

use orch_api_contract::background::ThreadBackgroundGetQuery;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::workspace::Job;

use orch_engine::background::BackgroundError;

use orch_engine::background::Identity;
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(ThreadBackgroundGet, operation, read::get)
        .on(ThreadBackgroundEnd, end);
}

fn identity(via: &Via) -> ApiResult<Identity> {
    Ok(match via {
        Via::OwnerRead { .. } => {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "A native owner read cannot create background authority",
            ));
        }
        Via::Session { session_id } => Identity {
            credential_id: *session_id,
            token: false,
        },
        Via::Token { token_id, .. } => Identity {
            credential_id: *token_id,
            token: true,
        },
    })
}

async fn failed(ctx: &Ctx, error: BackgroundError) -> ApiError {
    match error {
        BackgroundError::Scope => ApiError::new(
            ErrorCode::AuthzDenied,
            "the key does not allow this background read",
        ),
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

fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut orch_db::Conn,
    caller: &'a orch_authz::Caller,
    _query: ThreadBackgroundGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = crate::key_operations::admission::Prepared::new(ctx.route, ctx.route.name)?;
        *plan.shared_mut() = orch_engine::background::read_scope(conn, caller, ctx.id("id")?)
            .await
            .map_err(|error| match error {
                BackgroundError::Scope => ApiError::new(ErrorCode::AuthzDenied, error.to_string()),
                BackgroundError::NotFound => ApiError::not_found(),
                other => ApiError::internal(other),
            })?;
        Ok(plan)
    })
}

async fn end(ctx: Ctx, (): (), request: BackgroundEndRequest) -> ApiResult<Answer<Job>> {
    let thread = ctx.id("id")?;
    let (tx, guard) = transaction_guard::begin(&ctx).await?;
    let identity = identity(&guard.user().via)?;
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

/// Binds the roster read and the captured mutation request.
#[path = "background/read.rs"]
pub mod read;
