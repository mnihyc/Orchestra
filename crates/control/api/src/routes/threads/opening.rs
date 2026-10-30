//! One initial screen read; the existing native handlers remain the source of every projection.

use crate::middleware::authorization;
use crate::middleware::requests::ApiResult;
use crate::registry::{Answer, Ctx, Registry};
use http::header::CACHE_CONTROL;
use orch_api_contract::routes::{Access, Endpoint, Route};
use orch_api_contract::settings::{NodeSettingList, ThreadParameterGet};
use orch_api_contract::threads::{
    ThreadDraftGet, ThreadLimitsGet, ThreadLimitsGetQuery, ThreadOpening, ThreadOpeningGet,
    ThreadOpeningGetQuery, ThreadOutlineGetQuery, ThreadSyncGet, ThreadSyncGetQuery,
};
use std::future::Future;

/// Binds the initial-screen reader; incremental sync remains a separate native request.
pub(super) fn mount(registry: &mut Registry) {
    registry.on(ThreadOpeningGet, opening);
}

/// Preserve the caller and request, including the exact permission declared by the native reader.
async fn read<T, F, Fut>(ctx: &Ctx, route: Route, handler: F) -> ApiResult<T>
where
    F: FnOnce(Ctx) -> Fut,
    Fut: Future<Output = ApiResult<T>>,
{
    let mut child = ctx.clone();
    child.route = route;
    if let Access::Permission(permission) = route.access {
        authorization::require_on_path(&child, permission).await?;
    }
    // The permission read has released its connection. Each native handler then completes before
    // the next one starts: no transaction or connection spans an engine call or another acquire.
    handler(child).await
}

fn available<T>(result: ApiResult<T>) -> ApiResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.problem.status == 403 => Ok(None),
        Err(error) => Err(error),
    }
}

async fn opening(
    ctx: Ctx,
    query: ThreadOpeningGetQuery,
    (): (),
) -> ApiResult<Answer<ThreadOpening>> {
    // First admit the conversation, before looking up a draft or any related control.
    let mut child = ctx.clone();
    child.route = ThreadSyncGet::ROUTE;
    authorization::require_on_path(&child, "thread.view").await?;
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let sync = super::read::sync_in(
        &child,
        &mut tx,
        ThreadSyncGetQuery {
            limit: query.limit,
            continuation_id: query.continuation_id,
            projection_revision: query.projection_revision,
            ..Default::default()
        },
    )
    .await?;
    let outline = super::read::outline_in(
        &child,
        &mut tx,
        ThreadOutlineGetQuery {
            limit: query.limit,
            continuation_id: query.continuation_id,
            projection_revision: query.projection_revision,
            ..Default::default()
        },
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    authorization::require_on_path(&child, "thread.view").await?;
    let draft = available(
        read(&ctx, ThreadDraftGet::ROUTE, |ctx| {
            super::drive::draft_get(ctx, (), ())
        })
        .await,
    )?
    .flatten();
    let parameters = available(
        read(&ctx, ThreadParameterGet::ROUTE, |ctx| {
            super::parameters::parameters(ctx, (), ())
        })
        .await,
    )?;
    let limits = available(
        read(&ctx, ThreadLimitsGet::ROUTE, |ctx| {
            super::drive::limits_get(ctx, ThreadLimitsGetQuery::default(), ())
        })
        .await,
    )?;
    let settings = read(&ctx, NodeSettingList::ROUTE, |ctx| {
        crate::routes::settings::setting_list(ctx, (), ())
    })
    .await?;
    authorization::require_on_path(&child, "thread.view").await?;
    Ok(Answer::from(ThreadOpening {
        sync,
        draft,
        parameters,
        limits,
        settings,
        outline,
    })
    .header(CACHE_CONTROL, crate::sign_in::NOT_KEPT))
}
