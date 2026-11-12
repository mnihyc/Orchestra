//! Personal exceptions use stored subscriptions, one bounded View projection and the bell's mutation.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::nodes;

use base64::Engine as _;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::notifications::NotificationException;

use orch_api_contract::notifications::NotificationExceptionChanged;

use orch_api_contract::notifications::NotificationExceptionSetBody;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::notifications::exceptions;

use orch_types::Id;
pub(super) const CURSOR: &str = "notification-exceptions.v1:";

pub(super) fn after(query: &PageQuery) -> ApiResult<Option<Id>> {
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
        if let Some(path) = paths.remove(&row.node_id)
            && let Some(item) = project(&row, path)?
        {
            items.push(item);
        }
    }
    Ok(Page { items, next_cursor })
}

pub(super) async fn list(
    ctx: Ctx,
    query: PageQuery,
    (): (),
) -> ApiResult<Page<NotificationException>> {
    crate::key_operations::native::selected_read::<super::exception_read::Exceptions>(
        ctx,
        query,
        (),
    )
    .await
}

pub(super) fn project(
    row: &orch_db::queries::notifications::exceptions::ExceptionRow,
    mut path: Vec<orch_api_contract::nodes::NodeRef>,
) -> ApiResult<Option<NotificationException>> {
    let Some(node) = path
        .pop()
        .filter(|node| node.id == row.node_id && !node.redacted)
    else {
        return Ok(None);
    };
    let default_from = row
        .default_from
        .and_then(|id| path.iter().find(|node| node.id == id).cloned());
    Ok(Some(NotificationException {
        node,
        path,
        level: serde_json::from_value(row.level.clone().into()).map_err(ApiError::internal)?,
        default_level: serde_json::from_value(row.default_level.clone().into())
            .map_err(ApiError::internal)?,
        default_from,
    }))
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
