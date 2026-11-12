//! The existing delegated-role and registered-resource prerequisites, before any role command.

use crate::EngineError;
use orch_db::{
    Conn,
    queries::{environment_facts, resources},
};
use orch_link_contract::{
    enrollment_choices::connector_roles,
    policy::{LocalPolicy, PolicyKey},
    session::Role,
};
use orch_types::Id;

/// Inspect the native current roles, local delegation and actual registrations without rendering.
/// None means this native prerequisite is met; ordinary and key authority are checked separately.
pub async fn role_change_refusal(
    conn: &mut Conn,
    machine: Id,
    requested: &[String],
) -> Result<Option<&'static str>, EngineError> {
    let roles = connector_roles(&requested.join(",")).map_err(EngineError::unreadable)?;
    if roles.len() != requested.len() {
        return Ok(Some("Choose each role as a separate name."));
    }
    let row = resources::machine(conn, machine)
        .await?
        .ok_or_else(|| EngineError::unreadable("the role-selection machine disappeared"))?;
    if row.roles.iter().any(|role| role == "node") {
        return Ok(Some(
            orch_link_contract::enrollment_choices::CONNECTOR_NODE_REFUSAL,
        ));
    }
    let facts = environment_facts::machine(conn, machine, None).await?;
    let Some(value) = facts
        .as_ref()
        .and_then(|facts| facts.policy.as_ref())
        .filter(|value| !value.is_null())
    else {
        return Ok(Some(
            "This machine has not reported its role policy. Update its connector and try again.",
        ));
    };
    let policy: LocalPolicy = serde_json::from_value(value.clone())?;
    if !policy.delegates(PolicyKey::MachineRoles) || !policy.machine_roles {
        return Ok(Some(
            "machine.roles (machine section): the machine operator keeps role changes here or has switched them off. Use the role command on this machine, or hand machine.roles to Orch.",
        ));
    }
    let providers = resources::routes::providers_among(conn, &[], &[machine]).await?;
    let agents = resources::routes::agents_among(conn, &[], &[machine]).await?;
    let environments = resources::routes::environments_among(conn, &[], &[machine]).await?;
    for (role, name, used, detail) in [
        (
            Role::Environment,
            "environment",
            !environments.is_empty(),
            "Remove its registered environments before turning off Environment.",
        ),
        (
            Role::Agent,
            "agent",
            !agents.is_empty(),
            "Remove its registered agent profiles before turning off Agent.",
        ),
        (
            Role::Provider,
            "provider",
            !providers.is_empty(),
            "Remove its registered providers before turning off Provider.",
        ),
    ] {
        if used && row.roles.iter().any(|current| current == name) && !roles.contains(&role) {
            return Ok(Some(detail));
        }
    }
    Ok(None)
}
