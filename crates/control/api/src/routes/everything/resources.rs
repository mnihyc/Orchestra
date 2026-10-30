//! Resource-list filters and bounded rich rows, using the inventory's own frame and existing readers.

use std::collections::{HashMap, HashSet};

use crate::routes::resources::{
    agents, hosts, machines, mcp_readers, mounts, providers, templates,
};
use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::everything::{EverythingListQuery, EverythingResource, EverythingSort};
use orch_authz::{Principal, everything::Projection, resources::Seen};
use orch_db::{
    Conn,
    queries::everything::{Grant, Row},
};
use orch_types::Id;

/// These projections affect the shared SQL filter/order before its LIMIT, never a client-side slice.
pub(super) async fn prepare(
    conn: &mut Conn,
    query: &EverythingListQuery,
    projection: &mut Projection,
) -> ApiResult<()> {
    if query.resource_details == Some(true) {
        let names: std::collections::BTreeMap<_, _> = projection
            .names
            .iter()
            .filter_map(|(id, step)| step.name.as_ref().map(|name| (id.to_string(), name)))
            .collect();
        projection.grants["resource_names"] =
            serde_json::to_value(names).map_err(ApiError::internal)?;
    }
    if query.sort == Some(EverythingSort::Load) {
        let ids: Vec<_> = projection
            .resources
            .iter()
            .filter(|each| {
                each.resource.kind == ResourceKind::Host
                    && each
                        .held
                        .iter()
                        .any(|held| held.permission == Permission::HostView)
            })
            .map(|each| each.resource.id)
            .collect();
        projection.grants["host_loads"] = hosts::inventory_loads(conn, &ids).await?;
    }
    Ok(())
}

/// Only the selected readable page is hydrated. No whole-list reader or pooled acquire runs here.
pub(super) async fn page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    kind: &str,
    rows: &[Row],
    grants: &[Grant],
    projection: &Projection,
) -> ApiResult<HashMap<String, EverythingResource>> {
    let ids: HashSet<Id> = rows
        .iter()
        .zip(grants)
        .filter(|(_, grant)| grant.view)
        .map(|(row, _)| row.id.parse().map_err(ApiError::internal))
        .collect::<ApiResult<_>>()?;
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let seen: Vec<Seen> = projection
        .resources
        .iter()
        .filter(|each| each.resource.kind.as_str() == kind && ids.contains(&each.resource.id))
        .cloned()
        .collect();
    let values: Vec<EverythingResource> = match kind {
        "agent" => agents::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Agent(Box::new(row)))
            .collect(),
        "provider" => providers::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Provider(Box::new(row)))
            .collect(),
        "host" => hosts::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Host(Box::new(row)))
            .collect(),
        "environment" => crate::routes::environments::inventory(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Environment(Box::new(row)))
            .collect(),
        "template" => templates::inventory(conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Template(Box::new(row)))
            .collect(),
        "mcp_server" => mcp_readers::views(ctx, conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::McpServer(Box::new(row)))
            .collect(),
        "machine" => {
            let ids: Vec<_> = ids.into_iter().collect();
            let rows =
                orch_db::queries::resources::routes::physical_machines_among(conn, &ids).await?;
            machines::machine_views(ctx, conn, rows)
                .await?
                .into_iter()
                .map(|row| EverythingResource::Machine(Box::new(row)))
                .collect()
        }
        "skill" => crate::routes::resources::skills::inventory(conn, principal, &seen)
            .await?
            .into_iter()
            .map(|row| EverythingResource::Skill(Box::new(row)))
            .collect(),
        "mount" => {
            // Location visibility and source-removal policy stay in the mount's existing reader.
            let mut values = Vec::with_capacity(ids.len());
            for id in ids {
                values.push(EverythingResource::Mount(Box::new(
                    mounts::show(ctx, conn, principal, id).await?,
                )));
            }
            values
        }
        _ => return Err(ApiError::invalid("kinds", "Choose one resource kind.")),
    };
    Ok(values
        .into_iter()
        .map(|value| {
            let id = match &value {
                EverythingResource::Agent(row) => row.base.id,
                EverythingResource::Provider(row) => row.base.id,
                EverythingResource::Host(row) => row.base.id,
                EverythingResource::Environment(row) => row.base.id,
                EverythingResource::Machine(row) => row.base.id,
                EverythingResource::Mount(row) => row.base.id,
                EverythingResource::Template(row) => row.id,
                EverythingResource::McpServer(row) => row.id,
                EverythingResource::Skill(row) => row.id,
            };
            (id.to_string(), value)
        })
        .collect())
}
