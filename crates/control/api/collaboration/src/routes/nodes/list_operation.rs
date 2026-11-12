//! The existing two native selectors share current Node projection/source bounds.

use super::child_list_in;

use super::projection::Projection;

use super::tree;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::authorization;

use crate::middleware::inspection;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use orch_api_contract::common::Listed;

use orch_api_contract::common::Page;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeChildListQuery;

use orch_api_contract::nodes::Topic;

use orch_api_contract::nodes::TopicListQuery;

use orch_authz::Caller;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Time;
struct Prepared<T> {
    page: Page<Listed<T>>,
    response: OperationPlan,
    inspections: Vec<inspection::Info>,
}

pub(super) fn children<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: NodeChildListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !crate::key_operations::native::prepared_read(ctx, caller) {
            return Ok(None);
        }
        let selection = if ctx.inspect_deleted {
            "NodeChildList.inspect_deleted"
        } else {
            "NodeChildList.read"
        };
        let scope = Projection::new(ctx, caller, selection)?;
        let raw =
            authorization::load_principal(conn, caller.clone(), Time::new(ctx.app.clock.now()))
                .await?;
        let (window, inspections) = if ctx.inspect_deleted {
            inspection::children_in(ctx, &query, conn, &raw, Some(&scope)).await?
        } else {
            (
                child_list_in(ctx, &query, conn, &raw, Some(&scope)).await?,
                Vec::new(),
            )
        };
        let page = window.page;
        let (nodes, hidden): (Vec<_>, Vec<_>) =
            page.items
                .iter()
                .fold((Vec::new(), Vec::new()), |(mut nodes, mut hidden), row| {
                    match row {
                        Listed::Visible(row) => nodes.push(row.content.clone()),
                        Listed::Hidden(row) => hidden.push((**row).clone()),
                    }
                    (nodes, hidden)
                });
        let plan = scope.finish(conn, &nodes, &hidden, window.more).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Prepared {
            page,
            response,
            inspections,
        })))
    })
}

pub(super) fn topics<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: TopicListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !crate::key_operations::native::prepared_read(ctx, caller) {
            return Ok(None);
        }
        let scope = Projection::new(ctx, caller, "TopicList")?;
        let raw =
            authorization::load_principal(conn, caller.clone(), Time::new(ctx.app.clock.now()))
                .await?;
        let window = tree::topic_list_in(ctx, &query, conn, &raw, Some(&scope)).await?;
        let page = window.page;
        let (nodes, hidden): (Vec<_>, Vec<_>) =
            page.items
                .iter()
                .fold((Vec::new(), Vec::new()), |(mut nodes, mut hidden), row| {
                    match row {
                        Listed::Visible(row) => nodes.push(Node::Topic(row.content.clone())),
                        Listed::Hidden(row) => hidden.push((**row).clone()),
                    }
                    (nodes, hidden)
                });
        let plan = scope.finish(conn, &nodes, &hidden, window.more).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Prepared {
            page,
            response,
            inspections: Vec::new(),
        })))
    })
}

pub(super) async fn read_children(
    ctx: Ctx,
    query: NodeChildListQuery,
) -> ApiResult<Page<Listed<Node>>> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    // Ordinary/legacy callers still use the original selector and inspection audit path.
    if !crate::key_operations::native::prepared_read(&ctx, principal.caller()) {
        let result = if ctx.inspect_deleted {
            drop(conn);
            let page = inspection::children(ctx, query).await?;
            return Ok(Page {
                next_cursor: page.next_cursor,
                items: page.items.into_iter().map(Listed::visible).collect(),
            });
        } else {
            child_list_in(&ctx, &query, &mut conn, &principal, None)
                .await?
                .page
        };
        conn.commit().await.map_err(DbError::from)?;
        return Ok(result);
    }
    let prepared = ctx.operation_data::<Prepared<Node>>()?;
    let answer = super::projection::disclose_read(
        &ctx,
        &mut conn,
        principal.caller(),
        prepared.response.clone(),
        prepared.page.clone(),
        "the node list is no longer allowed",
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    for info in &prepared.inspections {
        let node = answer
            .items
            .iter()
            .find_map(|row| match row {
                Listed::Visible(row) if super::node_id(&row.content) == info.id => {
                    Some(&row.content)
                }
                _ => None,
            })
            .ok_or_else(|| ApiError::internal("a selected inspection lost its native node"))?;
        inspection::finish_child(&ctx, info, node).await?;
    }
    Ok(answer)
}

pub(super) async fn read_topics(ctx: Ctx, query: TopicListQuery) -> ApiResult<Page<Listed<Topic>>> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let answer = if crate::key_operations::native::prepared_read(&ctx, principal.caller()) {
        let prepared = ctx.operation_data::<Prepared<Topic>>()?;
        super::projection::disclose_read(
            &ctx,
            &mut conn,
            principal.caller(),
            prepared.response.clone(),
            prepared.page.clone(),
            "the node list is no longer allowed",
        )
        .await?
    } else {
        tree::topic_list_in(&ctx, &query, &mut conn, &principal, None)
            .await?
            .page
    };
    conn.commit().await.map_err(DbError::from)?;
    Ok(answer)
}
