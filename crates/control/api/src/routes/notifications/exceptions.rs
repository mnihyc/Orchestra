//! Personal exceptions use stored subscriptions, one bounded View projection and the bell's mutation.
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use crate::routes::nodes;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_api_contract::{
    common::{Page, PageQuery},
    notifications::{
        NotificationException, NotificationExceptionChanged, NotificationExceptionSetBody,
    },
};
use orch_authz::{Policy, Principal};
use orch_db::{Conn, DbError, queries::notifications::exceptions};
use orch_types::Id;

const CURSOR: &str = "notification-exceptions.v1:";
fn after(query: &PageQuery) -> ApiResult<Option<Id>> {
    query
        .cursor
        .as_ref()
        .map(|cursor| {
            let bad = || ApiError::invalid("cursor", "this is not a notification exception page");
            let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| bad())?;
            let text = std::str::from_utf8(&bytes).map_err(|_| bad())?;
            text.strip_prefix(CURSOR)
                .ok_or_else(bad)?
                .parse::<Id>()
                .map_err(|_| bad())
        })
        .transpose()
}
async fn page(
    conn: &mut Conn,
    principal: &Principal,
    query: &PageQuery,
) -> ApiResult<Page<NotificationException>> {
    let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
    let mut rows = exceptions::page(
        conn,
        principal.user(),
        after(query)?,
        i64::from(query.limit()) + 1,
    )
    .await?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next_cursor = if more {
        rows.last()
            .map(|row| URL_SAFE_NO_PAD.encode(format!("{CURSOR}{}", row.node_id)))
    } else {
        None
    };
    let ids = rows.iter().map(|row| row.node_id).collect::<Vec<_>>();
    let mut paths = nodes::view::viewable_ways(conn, principal, &ids, Policy::default()).await?;
    let mut items = Vec::new();
    for row in rows {
        let Some(mut path) = paths.remove(&row.node_id) else {
            continue;
        };
        let Some(node) = path
            .pop()
            .filter(|node| node.id == row.node_id && !node.redacted)
        else {
            continue;
        };
        let default_from = row
            .default_from
            .and_then(|id| path.iter().find(|node| node.id == id).cloned());
        items.push(NotificationException {
            node,
            path,
            level: serde_json::from_value(row.level.into()).map_err(ApiError::internal)?,
            default_level: serde_json::from_value(row.default_level.into())
                .map_err(ApiError::internal)?,
            default_from,
        });
    }
    Ok(Page { items, next_cursor })
}
pub(super) async fn list(
    ctx: Ctx,
    query: PageQuery,
    (): (),
) -> ApiResult<Page<NotificationException>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    page(&mut conn, &principal, &query).await
}
pub(super) async fn change(
    ctx: Ctx,
    query: PageQuery,
    body: NotificationExceptionSetBody,
) -> ApiResult<NotificationExceptionChanged> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = nodes::action(&ctx).await?;
    let access = nodes::change_subscription(&ctx, &mut tx, &principal, id, body.level).await?;
    let exceptions = page(&mut tx, &principal, &query).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(NotificationExceptionChanged {
        access,
        exceptions,
        page_cursor: query.cursor,
    })
}
