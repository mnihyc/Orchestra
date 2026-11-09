//! MCP presets listed with private permission-page destinations and read only with MCP View.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::Listed;
use orch_api_contract::resources::{self as api, McpServer};
use orch_authz::nodes::Page;
use orch_authz::{Policy, resources};
use orch_types::Id;

mod operation;

/// Binds the read requests, independently of the connectors that create or update presets.
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_with_optional_operation(api::McpServerList, operation::list, operation::read_list)
        .on_with_optional_operation(api::McpServerGet, operation::get, operation::read_get);
}

pub(crate) async fn views(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<McpServer>> {
    views_in(ctx, conn, principal, seen, None).await
}

async fn views_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    seen: &[resources::Seen],
    environment: Option<Id>,
) -> ApiResult<Vec<McpServer>> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let mut prepared = super::mcp::projection::prepare(
        conn,
        principal,
        &ids,
        Some(seen),
        super::mcp::projection::Style::Presets,
    )
    .await?;
    prepared.context = environment;
    super::mcp::projection::render(ctx, conn, principal, &prepared, None).await
}

async fn candidates(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
) -> ApiResult<Vec<resources::Seen>> {
    let mut after = None;
    let mut all = Vec::new();
    loop {
        let page = resources::visible(
            conn,
            principal,
            ResourceKind::McpServer,
            Page {
                after,
                limit: super::LIST_LIMIT,
            },
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        all.extend(page.items);
        after = page.next;
        if after.is_none() {
            break;
        }
    }
    Ok(all)
}

fn query_scope<'a>(
    conn: &'a mut orch_db::Conn,
    _principal: &'a orch_authz::Principal,
    mut seen: Vec<resources::Seen>,
    query: &'a api::McpServerListQuery,
) -> futures::future::BoxFuture<'a, ApiResult<Vec<resources::Seen>>> {
    Box::pin(async move {
        if let Some(org) = query.org_id {
            let homes: Vec<_> = seen.iter().filter_map(|each| each.resource.home).collect();
            let inside =
                orch_db::queries::resources::routes::homes_under(conn, org, &homes).await?;
            seen.retain(|each| each.resource.home.is_none_or(|home| inside.contains(&home)));
        }
        Ok(seen)
    })
}

async fn ordinary_list(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &api::McpServerListQuery,
) -> ApiResult<Vec<Listed<McpServer>>> {
    require_context(conn, principal, query.environment_id).await?;
    let seen = candidates(conn, principal).await?;
    let mut seen = query_scope(conn, principal, seen, query).await?;
    let private = super::hidden_items(conn, principal, &mut seen).await?;
    Ok(super::listed(
        views_in(ctx, conn, principal, &seen, query.environment_id).await?,
        private,
    ))
}

async fn require_context(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    environment: Option<Id>,
) -> ApiResult<()> {
    if let Some(id) = environment {
        super::require(
            conn,
            principal,
            ResourceKind::Environment,
            id,
            Permission::EnvironmentView,
        )
        .await?;
    }
    Ok(())
}

/// The same preset/imported view and native permissions for HTTP and a resource event.
pub(crate) async fn read_for(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    id: Id,
) -> ApiResult<McpServer> {
    if let Some(server) = orch_db::queries::resources::mcp::servers(conn, &[id])
        .await?
        .into_iter()
        .find(|server| server.environment_id.is_some())
    {
        super::mcp::require_row(conn, principal, &server, false).await?;
        return super::mcp::show(ctx, conn, principal, id).await;
    }
    super::require(
        conn,
        principal,
        ResourceKind::McpServer,
        id,
        Permission::McpView,
    )
    .await?;
    let seen = candidates(conn, principal)
        .await?
        .into_iter()
        .filter(|each| each.resource.id == id)
        .collect::<Vec<_>>();
    views(ctx, conn, principal, &seen)
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)
}
