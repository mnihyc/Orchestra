//! A resource change carries its native GET view, projected for the actual session receiving it.
//! No canonical database row, credential or another reader's access cut is forwarded.

use std::sync::Arc;

use super::server::Connection;

use super::Fail;

use super::Inner;

use crate::middleware::authentication::Caller;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::resources;

use crate::routes::resources::agents;

use crate::routes::resources::hosts;

use crate::routes::resources::machines;

use crate::routes::resources::mcp_readers;

use crate::routes::resources::mounts;

use crate::routes::resources::providers;

use crate::routes::resources::skills;

use crate::routes::resources::templates;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind as AccessKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::everything::EverythingResource;

use orch_api_contract::routes::Endpoint;

use orch_authz::Principal;

use orch_db::Conn;

use orch_realtime_contract::feeds::Feed;

use orch_realtime_contract::feeds::ResourceEvent;

use orch_realtime_contract::feeds::ResourceKind;

use orch_types::Id;

use orch_types::Json;
/// Availability was already checked by the subscription. Losing View clears only the rich view;
/// other admitted rights can still receive their separately checked private progress.
/// This read is nontransactional and uses one connection; passive native readers never ask a machine.
pub(crate) async fn project(
    inner: &Arc<Inner>,
    connection: &Connection,
    feed: &Feed,
    legacy: &Json,
) -> Result<Option<Json>, Fail> {
    let Feed::Resource { kind, id } = feed else {
        return Ok(None);
    };
    // Standalone realtime test/embedding instances may not attach an API application. Retain their old notice.
    let Some(app) = inner.app() else {
        return Ok(Some(legacy.clone()));
    };
    let mut conn = inner.db.acquire().await?;
    let Some((principal, authed)) =
        super::subscriptions::reader_for_connection(inner, &mut conn, connection).await?
    else {
        return Ok(None);
    };
    let route = match kind {
        ResourceKind::Provider => orch_api_contract::resources::ProviderGet::ROUTE,
        ResourceKind::Host => orch_api_contract::resources::HostGet::ROUTE,
        ResourceKind::Agent => orch_api_contract::resources::AgentGet::ROUTE,
        ResourceKind::Machine => orch_api_contract::resources::MachineGet::ROUTE,
        ResourceKind::Template => orch_api_contract::resources::TemplateGet::ROUTE,
        ResourceKind::McpServer => orch_api_contract::resources::McpServerGet::ROUTE,
        ResourceKind::Mount => orch_api_contract::mounts::MountGet::ROUTE,
        ResourceKind::Skill => orch_api_contract::skills::SkillGet::ROUTE,
    };
    let ctx = Ctx::for_tool(app, Caller::User(Arc::new(authed)), route, *id)
        .with_admitted_caller(principal.caller().clone())
        .map_err(Fail::other)?;
    match read(&ctx, &mut conn, &principal, *kind, *id).await {
        Ok(row) => serde_json::to_value(ResourceEvent::Changed {
            row: Some(row),
            view_revoked: false,
        })
        .map(Some)
        .map_err(Fail::other),
        Err(error) if error.code() == ErrorCode::Internal => Err(Fail::other(error)),
        Err(_) => serde_json::to_value(ResourceEvent::Changed {
            row: None,
            view_revoked: true,
        })
        .map(Some)
        .map_err(Fail::other),
    }
}

pub async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    kind: ResourceKind,
    id: Id,
) -> ApiResult<EverythingResource> {
    Ok(match kind {
        ResourceKind::Provider => {
            resources::require(
                conn,
                principal,
                AccessKind::Provider,
                id,
                Permission::ProviderView,
            )
            .await?;
            EverythingResource::Provider(Box::new(providers::show(ctx, conn, principal, id).await?))
        }
        ResourceKind::Host => {
            resources::require(conn, principal, AccessKind::Host, id, Permission::HostView).await?;
            // Host GET's optional background ports probe is deliberately outside its passive stored view.
            EverythingResource::Host(Box::new(hosts::show(ctx, conn, principal, id).await?))
        }
        ResourceKind::Agent => {
            resources::require(
                conn,
                principal,
                AccessKind::Agent,
                id,
                Permission::AgentView,
            )
            .await?;
            EverythingResource::Agent(Box::new(agents::show(ctx, conn, principal, id).await?))
        }
        ResourceKind::Template => {
            resources::require(
                conn,
                principal,
                AccessKind::Template,
                id,
                Permission::TemplateView,
            )
            .await?;
            EverythingResource::Template(Box::new(templates::show(conn, principal, id).await?))
        }
        ResourceKind::Mount => {
            resources::require(
                conn,
                principal,
                AccessKind::Mount,
                id,
                Permission::MountView,
            )
            .await?;
            EverythingResource::Mount(Box::new(mounts::show(ctx, conn, principal, id).await?))
        }
        ResourceKind::McpServer => EverythingResource::McpServer(Box::new(
            mcp_readers::read_for(ctx, conn, principal, id).await?,
        )),
        ResourceKind::Skill => {
            EverythingResource::Skill(Box::new(skills::show(conn, principal, id).await?))
        }
        ResourceKind::Machine => {
            // The Session context retains the native physical-machine/Host facade permission supplier.
            let row = machines::own_machine(ctx, conn, id).await?;
            EverythingResource::Machine(Box::new(
                machines::machine_with_readiness(ctx, conn, row).await?,
            ))
        }
    })
}
