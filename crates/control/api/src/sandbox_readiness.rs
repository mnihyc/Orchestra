//! Shared sandbox readiness projections for machine pages and blocked threads, using the request's connection.

use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::{
    environment::{EnvironmentRef, EnvironmentSandboxReadiness},
    resources::MachineRef,
};
use orch_authz::{
    Policy, Principal,
    resources::{KeyAbout, Keying, may_change_policy_key},
};
use orch_db::{
    Conn,
    queries::{environment_facts, environments::thread_engine as environments, resources},
};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};

/// Compare a visible environment's requested mode with the same facts and ceilings as turn admission.
pub(crate) async fn environment(
    conn: &mut Conn,
    principal: &Principal,
    reference: EnvironmentRef,
    place: Option<Place>,
    agent: Option<Id>,
    executor: Option<&str>,
) -> ApiResult<EnvironmentSandboxReadiness> {
    let row = environments::environment(conn, reference.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let physical = environment_facts::of(conn, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let template = environments::template_spec(conn, row.id).await?;
    let fallback = orch_settings::supplied::from_environment(
        keys::SANDBOX_MODE,
        physical.confinement(),
        template.as_ref(),
    )
    .ok_or_else(|| ApiError::internal("the environment has no sandbox default"))?;
    let requested = get(
        conn,
        place.unwrap_or(Place::Node(row.home_node_id)),
        keys::SANDBOX_MODE,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(serde_json::from_value(fallback).map_err(ApiError::internal)?);
    let descriptor = match agent {
        Some(id) => match resources::thread_engine::agent(conn, id).await? {
            Some(agent) => {
                orch_engine::harnesses::descriptor_of(&agent).map_err(ApiError::internal)?
            }
            None => None,
        },
        None => None,
    };
    let modes = descriptor
        .as_ref()
        .map(|descriptor| descriptor.sandbox_modes.as_slice());
    let mut candidates = Vec::new();
    if let Some(executor) = executor {
        candidates.push(orch_engine::sessions::sandbox_readiness(
            requested,
            modes,
            &row.facts,
            &physical,
            template.as_ref(),
            Some(executor),
        ));
    } else if agent.is_some() {
        candidates.push(orch_engine::sessions::sandbox_readiness(
            requested,
            modes,
            &row.facts,
            &physical,
            template.as_ref(),
            None,
        ));
    } else {
        let offers = orch_engine::binding::offers_of(conn, row.id)
            .await
            .map_err(ApiError::internal)?;
        for fact in row
            .facts
            .get("executors")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some(kind) = fact.get("kind").and_then(Json::as_str) else {
                continue;
            };
            let offered = offers.iter().any(|offer| offer == kind)
                || fact
                    .get("tool_sets")
                    .and_then(Json::as_array)
                    .is_some_and(|sets| {
                        sets.iter()
                            .filter_map(Json::as_str)
                            .any(|set| offers.iter().any(|offer| offer == set))
                    });
            if offered {
                candidates.push(orch_engine::sessions::sandbox_readiness(
                    requested,
                    None,
                    &row.facts,
                    &physical,
                    template.as_ref(),
                    Some(kind),
                ));
            }
        }
    }
    let sandbox = candidates
        .iter()
        .find(|candidate| candidate.ready)
        .cloned()
        .or_else(|| candidates.into_iter().next())
        .unwrap_or_else(|| {
            orch_engine::sessions::sandbox_readiness(
                requested,
                modes,
                &row.facts,
                &physical,
                template.as_ref(),
                executor,
            )
        });
    let (machine, policy_manage) = match physical.machine_id {
        Some(id) => {
            let row = resources::machine(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let manages = matches!(
                may_change_policy_key(
                    conn,
                    principal,
                    id,
                    KeyAbout::TheMachine,
                    true,
                    Policy::default()
                )
                .await
                .map_err(ApiError::internal)?,
                Keying::Allowed
            );
            let reference = MachineRef {
                id,
                name: row.name,
                online: row.online,
                placement: serde_json::from_value(Json::String(row.placement))
                    .map_err(ApiError::internal)?,
                platform_os: serde_json::from_value(Json::String(row.platform_os))
                    .map_err(ApiError::internal)?,
            };
            (Some(reference), manages)
        }
        None => (None, false),
    };
    Ok(EnvironmentSandboxReadiness {
        environment: reference,
        machine,
        policy_manage,
        sandbox,
    })
}
