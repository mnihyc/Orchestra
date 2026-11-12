//! Shared sandbox readiness projections for machine pages and blocked threads, using the request's connection.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_api_contract::environment::EnvironmentRef;

use orch_api_contract::environment::EnvironmentSandboxReadiness;

use orch_api_contract::environment::SandboxReadiness;

use orch_api_contract::resources::MachineRef;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::resources::KeyAbout;

use orch_authz::resources::Keying;

use orch_authz::resources::may_change_policy_key;

use orch_db::Conn;

use orch_db::queries::environment_facts;

use orch_db::queries::environments::thread_engine as environments;

use orch_db::queries::resources;

use orch_settings::Place;

use orch_settings::get;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;
/// Compare a visible environment's requested mode with the same facts and ceilings as turn admission.
pub async fn environment(
    conn: &mut Conn,
    principal: &Principal,
    reference: EnvironmentRef,
    place: Option<Place>,
    agent: Option<Id>,
    executor: Option<&str>,
) -> ApiResult<EnvironmentSandboxReadiness> {
    prepare(conn, principal, reference.id, place, agent, executor)
        .await?
        .render(reference)
}

/// Native readiness operands, before a caller's Environment reference is projected.
pub struct Prepared {
    environment: Id,
    machine: Option<resources::MachineRow>,
    policy_manage: bool,
    sandbox: SandboxReadiness,
}

impl Prepared {
    pub fn machine(&self) -> Option<Id> {
        self.machine.as_ref().map(|row| row.id)
    }

    /// Keep the physical association and any disclosed policy-management capability.
    pub fn preserved_by(&self, fresh: &Self) -> bool {
        self.environment == fresh.environment
            && self.machine() == fresh.machine()
            && (!self.policy_manage || fresh.policy_manage)
    }

    pub fn render(&self, reference: EnvironmentRef) -> ApiResult<EnvironmentSandboxReadiness> {
        if reference.id != self.environment {
            return Err(ApiError::internal(
                "the readiness reference changed environment",
            ));
        }
        let machine = self
            .machine
            .as_ref()
            .map(|row| -> ApiResult<MachineRef> {
                Ok(MachineRef {
                    id: row.id,
                    name: row.name.clone(),
                    online: row.online,
                    placement: serde_json::from_value(Json::String(row.placement.clone()))
                        .map_err(ApiError::internal)?,
                    platform_os: serde_json::from_value(Json::String(row.platform_os.clone()))
                        .map_err(ApiError::internal)?,
                })
            })
            .transpose()?;
        Ok(EnvironmentSandboxReadiness {
            environment: reference,
            machine,
            policy_manage: self.policy_manage,
            sandbox: self.sandbox.clone(),
        })
    }
}

/// Effect-free native source preparation, sharing the ordinary readiness algorithm.
pub async fn prepare(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    place: Option<Place>,
    agent: Option<Id>,
    executor: Option<&str>,
) -> ApiResult<Prepared> {
    let row = environments::environment(conn, environment)
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
            (Some(row), manages)
        }
        None => (None, false),
    };
    Ok(Prepared {
        environment,
        machine,
        policy_manage,
        sandbox,
    })
}
