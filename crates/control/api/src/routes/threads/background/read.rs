//! Actual HTTP read identity after native observation and source preparation, shared with page finalization.

use super::*;
use crate::key_operations::background_read::{Frame, mapped};
use crate::key_operations::snapshot_read;
use crate::middleware::transaction_guard::HttpIdentityMode;
use orch_authz::Caller;
use orch_db::Conn;
use orch_engine::background::{Capture, PreparedPage, Target};
use orch_types::Id;
use std::sync::Arc;

async fn frame(ctx: &Ctx, conn: &mut Conn, thread: Id, capture: &Capture) -> ApiResult<Frame> {
    let identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    crate::key_operations::background_read::prepare(&ctx.app, conn, identity, None, thread, capture)
        .await
}

struct Snapshot {
    frame: Frame,
    projected: ApiResult<(ThreadBackground, Vec<Target>)>,
}

async fn snapshot(ctx: Ctx, thread: Id, capture: Arc<Capture>) -> ApiResult<Snapshot> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let prepared = match ctx
        .app
        .engine
        .prepare_background_projection(&mut tx, thread, &capture)
        .await
    {
        Ok(prepared) => Ok(prepared),
        Err(BackgroundError::Internal(error)) => {
            let error = snapshot_read::engine_error(error);
            if error.is_database_retry() {
                return Err(error);
            }
            Err(error)
        }
        Err(error) => Err(mapped(&mut tx, error).await),
    };
    #[cfg(test)]
    crate::realtime::background_read_tests::at(
        thread,
        crate::realtime::background_read_tests::Cut::HttpPrepared,
    )
    .await;
    let reader = frame(&ctx, &mut tx, thread, &capture).await?;
    let projected = match prepared {
        Ok(prepared) => match ctx
            .app
            .engine
            .render_background_projection(
                &mut tx,
                &reader.principal,
                &prepared,
                &reader.directories,
            )
            .await
        {
            Ok(projected) => Ok(projected),
            Err(error) => Err(crate::key_operations::background_read::projection_error(
                &mut tx,
                &reader.principal,
                error,
            ).await),
        },
        Err(error) => Err(error),
    };
    #[cfg(test)]
    crate::realtime::background_read_tests::at(
        thread,
        crate::realtime::background_read_tests::Cut::HttpProjected,
    )
    .await;
    let committed = tx.commit().await.map_err(orch_db::DbError::from);
    reader.identity.check_deadline(&ctx.app)?;
    committed?;
    Ok(Snapshot {
        frame: reader,
        projected,
    })
}

pub(super) async fn get(
    ctx: Ctx,
    query: ThreadBackgroundGetQuery,
    (): (),
) -> ApiResult<ThreadBackground> {
    let thread = ctx.id("id")?;
    let user = ctx.user()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    authorization::require_on_path_in(&ctx, &mut conn, &principal, "thread.view").await?;
    let review_caller: Caller = if principal
        .caller()
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        principal.caller().clone()
    } else {
        authorization::caller_of(user)?
    };
    let identity = identity(&user.via);
    drop(conn);
    let page: PreparedPage = match ctx
        .app
        .engine
        .prepare_background_page(
            &review_caller,
            identity,
            thread,
            query.cursor.as_deref(),
            query.limit,
        )
        .await
    {
        Ok(page) => page,
        Err(error) => {
            let mut conn = ctx.app.db.acquire().await?;
            let identity =
                transaction_guard::current_http_identity(&ctx, &mut conn, HttpIdentityMode::Read)
                    .await?;
            let error = mapped(&mut conn, error).await;
            identity.check_deadline(&ctx.app)?;
            return Err(error);
        }
    };
    let capture = Arc::new(page.capture().clone());
    loop {
        let value = snapshot_read::run(ctx.clone(), {
            let capture = Arc::clone(&capture);
            move |ctx| snapshot(ctx, thread, Arc::clone(&capture))
        })
        .await?;
        let mut conn = ctx.app.db.acquire().await?;
        let current = frame(&ctx, &mut conn, thread, &capture).await?;
        current.identity.check_deadline(&ctx.app)?;
        if !value.frame.live(&ctx.app)
            || !current.live(&ctx.app)
            || !value.frame.same_context(&current)
        {
            // Re-render the same bounded observation under the new reader; never observe
            // the native session again or register a review from a discarded projection.
            continue;
        }
        let (roster, targets) = value.projected?;
        drop(conn);
        return ctx
            .app
            .engine
            .finish_background_page(page, roster, targets)
            .map_err(|error| match error {
                BackgroundError::Stale => ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the reviewed command list expired or changed; refresh it before End",
                )
                .with("refresh", "background"),
                BackgroundError::Invalid(message) => ApiError::invalid("", message),
                other => ApiError::internal(other),
            });
    }
}
