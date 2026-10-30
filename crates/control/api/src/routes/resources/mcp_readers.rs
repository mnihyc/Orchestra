//! MCP presets listed with private permission-page destinations and read only with MCP View.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::view as node_view;
use crate::routes::resources::view::{Holding, accesses, addresses, machine_refs};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::common::Listed;
use orch_api_contract::resources::{self as api, McpServer};
use orch_authz::nodes::Page;
use orch_authz::{Policy, resources};
use orch_types::{Id, Json};

/// Binds the read requests, independently of the connectors that create or update presets.
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on(api::McpServerList, list)
        .on(api::McpServerGet, get);
}

#[derive(sqlx::FromRow)]
struct Row {
    id: Id,
    name: String,
    description: String,
    org_id: Option<Id>,
    home_id: Option<Id>,
    owner_id: Option<Id>,
    machine_id: Option<Id>,
    spec: Json,
    enabled: bool,
    imported: bool,
    revision: i64,
}

pub(crate) async fn views(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    seen: &[resources::Seen],
) -> ApiResult<Vec<McpServer>> {
    let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
    let rows: Vec<Row> = sqlx::query_as("SELECT id,name,description,org_id,home_id,owner_id,machine_id,spec,enabled,imported,revision
        FROM mcp_servers WHERE id = ANY($1) AND environment_id IS NULL AND deleted_at IS NULL ORDER BY id")
        .bind(&ids).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?;
    let owners = refusals::user_refs(
        conn,
        &rows
            .iter()
            .filter_map(|row| row.owner_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let homes = node_view::refs(
        conn,
        principal,
        &rows
            .iter()
            .filter_map(|row| row.home_id)
            .collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let machines = machine_refs(
        &ctx.app,
        conn,
        &rows
            .iter()
            .filter_map(|row| row.machine_id)
            .collect::<Vec<_>>(),
    )
    .await?;
    let addresses = addresses(conn, &ids).await?;
    let holdings: Vec<_> = seen
        .iter()
        .map(|each| Holding {
            admin_only: &each.admin_only,
            admin_access: each.admin_access,
            admin_place: each.admin_place,
            participating: each.participating,
            id: each.resource.id,
            owner: each.owner,
            administers: each.administers,
            held: &each.held,
        })
        .collect();
    let access = accesses(conn, principal, &holdings).await?;
    let enabled: std::collections::HashMap<Id, i64> = sqlx::query_as::<_, (Id, i64)>(
        "SELECT wanted.id,
          (SELECT count(*) FROM settings s JOIN nodes n ON n.id=s.node_id AND n.deleted_at IS NULL WHERE s.key='mcp.enabled' AND s.value->>wanted.id::text='true') +
          (SELECT count(*) FROM environments e WHERE e.deleted_at IS NULL AND e.config->'mcp.enabled'->>wanted.id::text='true')
         FROM unnest($1::uuid[]) wanted(id)",
    ).bind(&ids).fetch_all(&mut *conn).await.map_err(orch_db::DbError::from)?.into_iter().collect();
    let mut imported_views = std::collections::HashMap::new();
    for row in rows.iter().filter(|row| row.imported) {
        imported_views.insert(
            row.id,
            super::mcp::show(ctx, conn, principal, row.id).await?,
        );
    }
    rows.into_iter().map(|row| {
        if let Some(mut view) = imported_views.remove(&row.id) {
            view.access = access.get(&row.id).cloned().ok_or_else(|| ApiError::internal("the MCP import's access was not read"))?;
            return Ok(view);
        }
        let spec = if row.machine_id.is_some() {
            serde_json::json!({"transport": row.spec.get("transport").cloned().unwrap_or(Json::Null)})
        } else { row.spec.clone() };
        Ok(McpServer {
            offerings: Vec::new(),
            enabled_places: Some(if row.enabled { enabled.get(&row.id).copied().unwrap_or_default() } else { 0 }),
            id: row.id, name: row.name, description: row.description, org_id: row.org_id,
            owner: row.owner_id.and_then(|id| owners.get(&id).cloned()),
            home: row.home_id.and_then(|id| homes.get(&id).cloned()),
            address: addresses.get(&row.id).cloned().unwrap_or_default(),
            machine: row.machine_id.and_then(|id| machines.get(&id).cloned()),
            environment: None,
            imported: None,
            spec, enabled: row.enabled, revision: row.revision,
            access: access.get(&row.id).cloned().ok_or_else(|| ApiError::internal("the MCP preset's access was not read"))?,
        })
    }).collect()
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

async fn list(
    ctx: Ctx,
    query: api::McpServerListQuery,
    (): (),
) -> ApiResult<Vec<Listed<McpServer>>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut seen = candidates(&mut conn, &principal).await?;
    if let Some(org) = query.org_id {
        let homes: Vec<_> = seen.iter().filter_map(|each| each.resource.home).collect();
        let inside =
            orch_db::queries::resources::routes::homes_under(&mut conn, org, &homes).await?;
        seen.retain(|each| each.resource.home.is_none_or(|home| inside.contains(&home)));
    }
    let private = super::hidden_items(&mut conn, &principal, &mut seen).await?;
    Ok(super::listed(
        views(&ctx, &mut conn, &principal, &seen).await?,
        private,
    ))
}

async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<McpServer> {
    let id = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    read_for(&ctx, &mut conn, &principal, id).await
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
