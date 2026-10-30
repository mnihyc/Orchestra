//! Local machine requests carried by its authenticated link and authorized as its current owner.

use crate::middleware::authentication::{Authed, Caller, Via};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::state::{App, AppState};
use async_trait::async_trait;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::environment::{EnvironmentMcpImportCreate, EnvironmentMcpImportCreateBody};
use orch_api_contract::routes::Endpoint;
use orch_db::queries::{identity, resources, transaction_guard};
use orch_link_contract::mcp_import::{McpImportAnswer, McpImportAsk};
use orch_types::Id;
use std::sync::{Arc, Weak};

/// The application hook called only after the backplane pins the authenticated machine/session pair.
#[async_trait]
pub trait Handler: Send + Sync {
    /// Files the import through the ordinary environment import producer, never a machine-chosen user.
    async fn import(&self, machine: Id, request: McpImportAsk) -> McpImportAnswer;
    /// A scoped read/peer refresh, never a person-selected identity or port-opening decision.
    async fn local_ports(
        &self,
        _machine: Id,
        request: orch_link_contract::local_ports::PortLocalAsk,
    ) -> orch_link_contract::local_ports::PortLocalAnswer {
        orch_link_contract::local_ports::PortLocalAnswer::unavailable(
            request.request_id,
            "inside port reads are unavailable",
        )
    }
}

struct Requests {
    app: Weak<AppState>,
    port_reads: crate::local_ports::Pending,
}

/// Attaches the adapter without a strong application/link reference cycle.
pub fn install(app: &App) {
    app.link.serve_machine_requests(Arc::new(Requests {
        app: Arc::downgrade(app),
        port_reads: Default::default(),
    }));
}

pub(crate) fn unavailable(id: Id, detail: &str) -> McpImportAnswer {
    McpImportAnswer {
        request_id: id,
        server_id: None,
        code: Some("unavailable".to_owned()),
        detail: detail.to_owned(),
    }
}

#[async_trait]
impl Handler for Requests {
    async fn local_ports(
        &self,
        machine: Id,
        request: orch_link_contract::local_ports::PortLocalAsk,
    ) -> orch_link_contract::local_ports::PortLocalAnswer {
        let Some(_permit) = self.port_reads.enter(request.environment_id) else {
            return orch_link_contract::local_ports::PortLocalAnswer::unavailable(
                request.request_id,
                "too many pending reads for this environment",
            );
        };
        let Some(app) = self.app.upgrade() else {
            return orch_link_contract::local_ports::PortLocalAnswer::unavailable(
                request.request_id,
                "the system is stopping",
            );
        };
        crate::local_ports::answer(&app, machine, request).await
    }
    async fn import(&self, machine: Id, request: McpImportAsk) -> McpImportAnswer {
        let Some(app) = self.app.upgrade() else {
            return unavailable(request.request_id, "the system is stopping");
        };
        import_from_machine(&app, machine, request).await
    }
}

/// Applies a request whose machine identity came from the live link, never from its payload.
/// A non-browser owner principal has ordinary holdings and no system-admin override.
pub async fn import_from_machine(app: &App, machine: Id, request: McpImportAsk) -> McpImportAnswer {
    let id = request.request_id;
    match file(app, machine, request).await {
        Ok(server) => McpImportAnswer {
            request_id: id,
            server_id: Some(server),
            code: None,
            detail: format!("Imported MCP server {server}; its copy is kept on this machine."),
        },
        Err(error) => McpImportAnswer {
            request_id: id,
            server_id: None,
            code: Some(error.code().as_str().to_owned()),
            detail: error.problem.detail,
        },
    }
}

async fn file(app: &App, machine: Id, request: McpImportAsk) -> ApiResult<Id> {
    let mut tx = app.db.begin().await?;
    // Filing its permission page changes authority. Take the exclusive epoch first, then pin
    // the machine and its current owner for the entire ordinary import transaction.
    transaction_guard::lock_authority(&mut tx).await?;
    // SHARE blocks an owner/revocation change but permits the command row's foreign-key KEY SHARE.
    let _ = sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
        .bind(machine)
        .fetch_optional(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    let machine_row = resources::machine(&mut tx, machine)
        .await?
        .filter(|row| {
            row.placement == "connected" && row.status == "active" && row.deleted_at.is_none()
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "only an active connected machine imports as its owner",
            )
        })?;
    let owner = machine_row.owner_id.ok_or_else(|| {
        ApiError::new(
            ErrorCode::AuthzDenied,
            "this machine has no person as owner",
        )
    })?;
    let _ = sqlx::query("SELECT id FROM users WHERE id=$1 FOR SHARE")
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    let user = identity::user_by_id(&mut tx, owner)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this machine's owner is suspended",
        ));
    }
    let now = app.clock.time();
    let authed = Authed {
        user,
        as_admin: false,
        via: Via::Token {
            token_id: Id::nil(),
            scopes: vec!["*".to_owned()],
            node_id: None,
        },
    };
    // This value is for projections/audit only, never passed to an HTTP credential guard.
    let principal = crate::middleware::authorization::principal_of(&mut tx, &authed, now).await?;
    let environment =
        orch_db::queries::environments::hosting::environment(&mut tx, request.environment_id)
            .await?
            .filter(|env| env.machine_id == Some(machine) && env.deleted_at.is_none())
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the requested environment is not registered on this machine",
                )
            })?;
    let mut ctx = Ctx::for_tool(
        app.clone(),
        Caller::User(Arc::new(authed)),
        EnvironmentMcpImportCreate::ROUTE,
        environment.id,
    );
    ctx.request_id = format!("machine-cli:{machine}:{}", request.request_id);
    ctx.now = now.get();
    let server = crate::routes::environments::mcp::import_in(
        &ctx,
        &mut tx,
        &principal,
        environment.id,
        EnvironmentMcpImportCreateBody {
            source: request.source,
            scope: request.scope,
            name: request.name,
            directory: request.directory,
            as_name: None,
            enable: Some(false),
        },
        request.request_id,
        true,
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(server.id)
}
