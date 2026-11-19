//! Effective gateway metadata and observed carrier facts use the ordinary operation guards.

use crate::certificates::{
    GATEWAY_CATALOGUE_SETTING, gateway_catalogue_revision, validate_gateway_catalogue,
};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::gateways as api;
use orch_db::audit::{self, Actor, Entry};
use orch_db::{
    DbError,
    queries::{settings, threads},
};
use orch_link_contract::enrollment::{Endpoint, Trust};
use orch_link_contract::gateways::{
    GatewayInstance, GatewaySelection, GatewayStatusReport, TunnelPlan,
};

pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            api::GatewayCatalogueGet,
            crate::key_operations::direct::declared,
            catalogue,
        )
        .on_with_operation(
            api::GatewayCatalogueUpdate,
            crate::key_operations::direct::declared,
            update,
        )
        .on_with_operation(
            api::MachineGatewayGet,
            crate::key_operations::direct::declared,
            machine,
        )
        .on_with_operation(
            api::ThreadTunnelGatewayGet,
            crate::key_operations::direct::declared,
            thread,
        );
}

fn endpoint(endpoint: &Endpoint) -> Option<api::GatewayEndpoint> {
    let endpoint = orch_link_contract::gateways::public_endpoint(endpoint)?;
    Some(api::GatewayEndpoint {
        url: endpoint.url,
        trust: match endpoint.trust {
            Trust::Pinned => api::GatewayEndpointTrust::Pinned,
            Trust::Public => api::GatewayEndpointTrust::Public,
        },
    })
}

fn entry(gateway: GatewayInstance) -> api::GatewayCatalogueEntry {
    api::GatewayCatalogueEntry {
        id: gateway.id,
        label: gateway.label,
        region: gateway.region,
        endpoints: gateway.endpoints.iter().filter_map(endpoint).collect(),
    }
}

fn wire_entry(gateway: api::GatewayCatalogueEntry) -> GatewayInstance {
    GatewayInstance {
        id: gateway.id,
        label: gateway.label,
        region: gateway.region,
        endpoints: gateway
            .endpoints
            .into_iter()
            .map(|endpoint| Endpoint {
                url: endpoint.url,
                trust: match endpoint.trust {
                    api::GatewayEndpointTrust::Pinned => Trust::Pinned,
                    api::GatewayEndpointTrust::Public => Trust::Public,
                },
            })
            .collect(),
    }
}

fn plan(plan: TunnelPlan) -> api::TunnelGatewayPlan {
    api::TunnelGatewayPlan {
        plan_id: plan.plan_id,
        src: plan.src,
        dst: plan.dst,
        preferred_gateway: entry(plan.preferred_gateway),
        common_fallbacks: plan.common_fallbacks.into_iter().map(entry).collect(),
        catalogue_revision: plan.catalogue_revision,
    }
}

async fn catalogue(ctx: Ctx, (): (), (): ()) -> ApiResult<api::GatewayCatalogue> {
    let mut conn = ctx.app.db.acquire().await?;
    authorization::principal(&ctx, &mut conn).await?;
    let gateways = ctx.app.link.certificates().advertised_gateways();
    Ok(api::GatewayCatalogue {
        revision: gateway_catalogue_revision(&gateways),
        gateways: gateways.into_iter().map(entry).collect(),
    })
}

async fn update(
    ctx: Ctx,
    (): (),
    body: api::GatewayCatalogueUpdateBody,
) -> ApiResult<api::GatewayCatalogueUpdated> {
    ctx.ordinary_system_admin()?;
    let gateways: Vec<_> = body.gateways.into_iter().map(wire_entry).collect();
    validate_gateway_catalogue(&gateways)
        .map_err(|error| ApiError::invalid("gateways", &error.to_string()))?;
    let service = ctx.app.link.certificates();
    let _writer = service.catalogue_writer().await;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.ordinary_system_admin_with(guard.user(), guard.principal().caller())?;
    let previous = settings::row(&mut tx, None, GATEWAY_CATALOGUE_SETTING).await?;
    let current = match &previous {
        Some(row) => serde_json::from_value(row.value.clone()).map_err(ApiError::internal)?,
        None => service.configured_gateways().to_vec(),
    };
    if gateway_catalogue_revision(&current) != body.expected_revision {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the gateway catalogue changed since it was read",
        ));
    }
    let revision = gateway_catalogue_revision(&gateways);
    if gateways != current {
        let value = serde_json::to_value(&gateways).map_err(ApiError::internal)?;
        let written = settings::put(
            &mut tx,
            None,
            GATEWAY_CATALOGUE_SETTING,
            &settings::Stored {
                value: &value,
                locked: false,
                by: Some(guard.user().id()),
                at: guard.now().get(),
            },
            Some(previous.as_ref().map_or(0, |row| row.revision)),
        )
        .await?;
        if written == settings::Put::Stale {
            return Err(ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the gateway catalogue changed since it was read",
            ));
        }
        let mut audit = Entry::new(
            Actor::User(guard.user().id()),
            orch_access_contract::audit::AuditAction::SystemSettingsUpdated.as_str(),
            "system",
            None,
            "Gateway installation metadata updated",
        );
        audit.details = serde_json::json!({"key":GATEWAY_CATALOGUE_SETTING,"revision":revision,"gateways":gateways.iter().map(|gateway| &gateway.id).collect::<Vec<_>>()});
        audit::append(
            &mut tx,
            &audit,
            &crate::sign_in::origin(&ctx),
            guard.now().get(),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    service.replace_gateway_catalogue(gateways);
    if let Err(error) = ctx.app.link.publish_gateway_catalogue().await {
        tracing::warn!(%error, "persisted gateway metadata will be delivered on the next reconnect");
    }
    Ok(api::GatewayCatalogueUpdated { revision })
}

async fn machine(ctx: Ctx, (): (), (): ()) -> ApiResult<api::ConnectorGatewayStatus> {
    let id = ctx.id("machine_id")?;
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let row = crate::routes::resources::machines::own_machine(&ctx, &mut tx, id).await?;
    let report: Option<GatewayStatusReport> = row
        .facts
        .get("gateway")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok());
    let primary = ctx.app.link.gateway_of(id);
    let mut status = api::ConnectorGatewayStatus {
        machine_id: id,
        selection: None,
        selected_gateway_id: None,
        connected: primary.is_some(),
        primary_gateway_id: primary,
        measured_at: None,
        measurements: Vec::new(),
    };
    if let Some(report) = report {
        status.selection = Some(match report.choice.selection {
            GatewaySelection::Automatic => api::ConnectorGatewaySelection::Automatic,
            GatewaySelection::Selected => api::ConnectorGatewaySelection::Selected,
        });
        status.selected_gateway_id = report.choice.gateway_id;
        status.measured_at = report.measured_at;
        status.measurements = report
            .measurements
            .into_iter()
            .filter_map(|sample| {
                Some(api::GatewayLatency {
                    gateway_id: sample.gateway_id,
                    endpoint: endpoint(&sample.endpoint)?,
                    round_trip_ms: sample.round_trip_ms,
                })
            })
            .collect();
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(status)
}

async fn thread(ctx: Ctx, (): (), (): ()) -> ApiResult<api::ThreadTunnelGatewayStatus> {
    let id = ctx.id("thread_id")?;
    let mut tx = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let status = thread_in(&ctx, &mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(status)
}

/// The native carrier reader, reusable inside the admitted current thread opening.
pub(crate) async fn thread_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    id: orch_types::Id,
) -> ApiResult<api::ThreadTunnelGatewayStatus> {
    crate::routes::threads::one(ctx, conn, id).await?;
    let current = threads::thread(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut status = api::ThreadTunnelGatewayStatus {
        thread_id: id,
        state: api::TunnelGatewayState::Unknown,
        source_primary_gateway_id: None,
        destination_primary_gateway_id: None,
        attached_gateway_id: None,
        plan: None,
        refusal: None,
    };
    let source = ctx
        .app
        .engine
        .harness_machine_of(conn, id)
        .await
        .map_err(ApiError::internal)?;
    let environment = match current.environment_id {
        Some(environment) => {
            orch_db::queries::environments::thread_engine::environment(conn, environment).await?
        }
        None => None,
    };
    if let (Some(source), Some(environment)) = (source, environment) {
        status.source_primary_gateway_id = ctx.app.link.gateway_of(source);
        if let Some(destination) = environment.machine_id {
            status.destination_primary_gateway_id = ctx.app.link.gateway_of(destination);
            if source == destination {
                status.state = api::TunnelGatewayState::Local;
            } else if let Some(observed) = ctx.app.link.tool_gateway_status(
                id,
                source,
                destination,
                environment.id,
                i64::from(environment.generation),
            ) {
                status.attached_gateway_id = observed.gateway;
                status.plan = observed.plan.map(plan);
                status.refusal = observed.refusal.map(|(detail, attempted_gateways)| {
                    api::TunnelGatewayRefusal {
                        code: ErrorCode::TunnelGatewayUnreachable,
                        detail,
                        attempted_gateways,
                    }
                });
                status.state = if status.refusal.is_some() {
                    api::TunnelGatewayState::Unavailable
                } else if status.attached_gateway_id.is_some() {
                    api::TunnelGatewayState::Attached
                } else if status.plan.is_some() {
                    api::TunnelGatewayState::Pending
                } else {
                    api::TunnelGatewayState::Unknown
                };
            }
        }
    }
    Ok(status)
}
