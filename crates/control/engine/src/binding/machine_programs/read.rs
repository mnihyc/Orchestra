//! Read once for a fixed binding, or once for a whole set of binding options.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use orch_db::queries::environments::thread_engine::{self as environments, EnvironmentRow};
use orch_db::queries::resources::thread_engine::{self as resources, MachinePlace};
use orch_link_contract::session::MachineFactsHarness;
use orch_settings_contract::version_override::VersionOverrides;
use orch_types::{Id, Json};

use crate::EngineError;

/// Request-local facts for one environment; these are neither stored decisions nor new resources.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub environment: Id,
    /// Empty still means this no-cache machine was checked: old environment reports cannot restore removal.
    pub programs: BTreeMap<String, MachineFactsHarness>,
    pub machine: String,
    pub overrides: VersionOverrides,
    versions: BTreeMap<(Id, Id, Id), Option<String>>,
}

impl Prepared {
    pub fn agent_version(&self, provider: Id, agent: Id, machine: Option<Id>) -> Option<&str> {
        self.versions
            .get(&(provider, agent, machine?))
            .and_then(Option::as_deref)
            .filter(|version| !version.is_empty())
    }
}

fn confirmed_programs(machine: &MachinePlace) -> BTreeMap<String, MachineFactsHarness> {
    let mut confirmed = BTreeMap::new();
    let Some(policy) = machine
        .facts
        .get("policy")
        .and_then(|policy| policy.get("agent.harnesses"))
        .and_then(Json::as_array)
    else {
        return confirmed;
    };
    for program in machine
        .facts
        .get("harnesses")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| serde_json::from_value::<MachineFactsHarness>(entry.clone()).ok())
    {
        let native_executor = orch_agent_programs::Programs::builtin()
            .program(&program.kind)
            .is_some_and(|manifest| {
                manifest.executors.iter().any(|executor| {
                    super::executor_program(executor)
                        .is_some_and(|owner| owner.kind == program.kind)
                })
            });
        if !native_executor {
            continue;
        }
        let selected = policy
            .iter()
            .find(|entry| entry.get("kind").and_then(Json::as_str) == Some(program.kind.as_str()))
            .and_then(|entry| entry.get("path"))
            .and_then(Json::as_str);
        if selected.is_some_and(|selected| {
            !std::path::Path::new(selected).is_absolute() || selected == program.path
        }) {
            confirmed.entry(program.kind.clone()).or_insert(program);
        }
    }
    confirmed
}

/// Keys are exact provider/profile/actual-harness-host tuples prepared by the caller.
/// Missing reports remain unknown without falling back to a profile or an adapter declaration.
pub async fn load(
    conn: &mut orch_db::Conn,
    environment: &EnvironmentRow,
    machine: &MachinePlace,
    keys: &[(Id, Id, Option<Id>)],
) -> Result<Option<Arc<Prepared>>, EngineError> {
    if environment.placement != "connected"
        || environment.machine_id != Some(machine.id)
        || keys.is_empty()
    {
        return Ok(None);
    }
    let programs = confirmed_programs(machine);
    let mut versions = BTreeMap::new();
    let metadata =
        if let [(provider, agent, host)] = keys {
            let found = environments::machine_program_binding(
                conn,
                environment.id,
                machine.id,
                *provider,
                *agent,
                *host,
            )
            .await?;
            found.map(|(config, name, version)| {
                if let Some(host) = host {
                    versions.insert((*provider, *agent, *host), version);
                }
                (config, name)
            })
        } else {
            let found =
                environments::machine_program_metadata(conn, environment.id, machine.id).await?;
            if found.is_some() && !programs.is_empty() {
                let exact: BTreeSet<_> = keys
                    .iter()
                    .filter_map(|(provider, agent, machine)| {
                        machine.map(|machine| (*provider, *agent, machine))
                    })
                    .collect();
                if !exact.is_empty() {
                    let exact: Vec<_> = exact.into_iter().collect();
                    let reports = resources::harness_versions_by_key(conn, &exact).await?;
                    versions.extend(reports.into_iter().map(
                        |(provider, agent, machine, version)| ((provider, agent, machine), version),
                    ));
                }
            }
            found
        };
    Ok(metadata.map(|(config, machine)| {
        Arc::new(Prepared {
            environment: environment.id,
            programs,
            machine,
            overrides: config
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default(),
            versions,
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_common::World;
    use orch_db::queries::resources::{access_fixtures as fixtures, thread_engine as resources};
    use serde_json::json;

    async fn connected(world: &World) -> (EnvironmentRow, MachinePlace) {
        let mut conn = world.pool.acquire().await.unwrap();
        let environment = fixtures::insert_environment(
            &mut conn,
            world.ada,
            Some(world.root),
            "CodeX workstation",
            "connected",
        )
        .await
        .unwrap();
        let machine: Id = sqlx::query_scalar("SELECT machine_id FROM environments WHERE id=$1")
            .bind(environment)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        sqlx::query("UPDATE machines SET facts=$2 WHERE id=$1").bind(machine).bind(json!({
            "policy":{"agent.harnesses":[{"kind":"codex","path":"/controlled/codex"}]},
            "harnesses":[{"kind":"codex","path":"/controlled/codex","version":"1.0.0","daemon":false}]
        })).execute(&mut *conn).await.unwrap();
        (
            orch_db::queries::environments::thread_engine::environment(&mut conn, environment)
                .await
                .unwrap()
                .unwrap(),
            resources::machine_place(&mut conn, machine)
                .await
                .unwrap()
                .unwrap(),
        )
    }

    #[tokio::test]
    async fn the_fixed_lookup_uses_the_requested_runtime_host_and_the_environment_machine_name() {
        let world = World::observed().await;
        let thread = world.thread("Reported runtime").await;
        world
            .running_turn(thread, "Report the real fixture build")
            .await;
        world.run_turn(thread, 0, "Done").await;
        let (environment, machine) = connected(&world).await;
        let mut conn = world.pool.acquire().await.unwrap();
        // Another retained host report must not stand in for the selected provider runtime.
        sqlx::query("INSERT INTO harnesses(provider_id,agent_id,machine_id,harness_version) VALUES($1,$2,$3,'9.9.9')")
            .bind(world.provider).bind(world.agent).bind(machine.id).execute(&mut *conn).await.unwrap();
        let prepared = load(
            &mut conn,
            &environment,
            &machine,
            &[(world.provider, world.agent, Some(world.machine_id))],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(prepared.machine, "CodeX workstation");
        assert_eq!(
            prepared.programs.get("codex").unwrap().path,
            "/controlled/codex"
        );
        assert_eq!(
            prepared.agent_version(world.provider, world.agent, Some(world.machine_id)),
            Some("1.0.0")
        );
        let missing = load(
            &mut conn,
            &environment,
            &machine,
            &[(world.provider, world.agent, None)],
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            missing
                .agent_version(world.provider, world.agent, None)
                .is_none()
        );
    }

    #[tokio::test]
    async fn the_batch_reads_only_exact_keys_and_keeps_missing_reports_unknown() {
        let world = World::observed().await;
        let thread = world.thread("Exact build keys").await;
        world
            .running_turn(thread, "Report the real fixture build")
            .await;
        world.run_turn(thread, 0, "Done").await;
        let (environment, machine) = connected(&world).await;
        let mut conn = world.pool.acquire().await.unwrap();
        let provider = fixtures::insert_resource(
            &mut conn,
            "provider",
            Some(world.ada),
            Some(world.root),
            "Second runtime",
        )
        .await
        .unwrap();
        let agent = fixtures::insert_resource(
            &mut conn,
            "agent",
            Some(world.ada),
            Some(world.root),
            "Second profile",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE providers SET machine_id=$2 WHERE id=$1")
            .bind(provider)
            .bind(machine.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("INSERT INTO harnesses(provider_id,agent_id,machine_id,harness_version) VALUES($1,$2,$3,'9.9.9')")
            .bind(world.provider).bind(agent).bind(machine.id).execute(&mut *conn).await.unwrap();
        let prepared = load(
            &mut conn,
            &environment,
            &machine,
            &[
                (world.provider, world.agent, Some(world.machine_id)),
                (provider, agent, Some(machine.id)),
            ],
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            prepared.agent_version(world.provider, world.agent, Some(world.machine_id)),
            Some("1.0.0")
        );
        assert!(
            prepared
                .agent_version(provider, agent, Some(machine.id))
                .is_none()
        );
        assert!(
            !prepared
                .versions
                .contains_key(&(world.provider, agent, machine.id)),
            "independent key sets must not read unrelated combinations"
        );
        assert!(prepared.overrides.is_empty());
        let managed = orch_db::queries::environments::thread_engine::environment(
            &mut conn,
            world.environment,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            load(
                &mut conn,
                &managed,
                &machine,
                &[(world.provider, world.agent, Some(world.machine_id))]
            )
            .await
            .unwrap()
            .is_none()
        );
    }
}
