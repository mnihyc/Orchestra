//! Preparing environment consent for exactly the machine program the manager saw.

use orch_types::{Id, Json, Time};
use serde_json::{Map, json};

const KEY: &str = "exec.version_overrides";

pub(super) fn stored(config: &Json) -> orch_settings_contract::version_override::VersionOverrides {
    config
        .get(KEY)
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Only current policy-confirmed rows are a source for consent, never discoveries or installed caches.
pub(crate) fn confirmed(facts: &Json) -> Vec<orch_link_contract::session::MachineFactsHarness> {
    let Some(policy) = facts.get("policy").filter(|policy| policy.is_object()) else {
        return Vec::new();
    };
    let mut selected = std::collections::BTreeMap::new();
    for entry in policy
        .get("agent.harnesses")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(kind), Some(path)) = (
            entry.get("kind").and_then(Json::as_str),
            entry.get("path").and_then(Json::as_str),
        ) {
            selected.entry(kind).or_insert(path);
        }
    }
    facts
        .get("harnesses")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            serde_json::from_value::<orch_link_contract::session::MachineFactsHarness>(
                entry.clone(),
            )
            .ok()
        })
        .filter(|program| {
            selected.get(program.kind.as_str()).is_some_and(|selected| {
                !std::path::Path::new(selected).is_absolute() || *selected == program.path.as_str()
            })
        })
        .collect()
}

pub(super) fn view(
    config: &Json,
    facts: &Json,
    people: &std::collections::HashMap<Id, orch_api_contract::users::UserRef>,
) -> Vec<orch_api_contract::environment::EnvironmentMachineProgram> {
    let overrides = stored(config);
    confirmed(facts)
        .into_iter()
        .filter(|program| {
            orch_agent_programs::Programs::builtin()
                .executors()
                .any(|executor| {
                    executor.wire == orch_agent_programs::executors::ExecutorWire::Program
                        && executor.program.as_deref() == Some(program.kind.as_str())
                })
        })
        .map(|program| {
            let approval = overrides
                .get(&program.kind)
                .filter(|entry| entry.path == program.path && entry.version == program.version)
                .map(|entry| {
                    orch_api_contract::environment::EnvironmentProgramApproval {
                        // Keep the same empty-name reference used for historical usage writers.
                        // Losing the person record does not revoke a path/version decision.
                        by: people.get(&entry.by).cloned().unwrap_or_else(|| {
                            orch_api_contract::users::UserRef {
                                id: entry.by,
                                kind: orch_api_contract::users::UserKind::Human,
                                username: String::new(),
                                address: None,
                                display_name: String::new(),
                                avatar: orch_api_contract::users::Avatar {
                                    text: String::new(),
                                    color: 0,
                                },
                            }
                        }),
                        at: entry.at,
                    }
                });
            orch_api_contract::environment::EnvironmentMachineProgram {
                harness_kind: program.kind,
                path: program.path,
                version: program.version,
                approval,
            }
        })
        .collect()
}

/// Prepare the config value before registry validation. Attribution is always written by the server;
/// unchanged decisions keep the person and time who originally made them.
pub(super) fn prepare(
    config: &mut Json,
    before: &Json,
    machine_facts: &Json,
    by: Id,
    at: Time,
) -> Result<bool, &'static str> {
    let old = before
        .get(KEY)
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    let values = config
        .as_object_mut()
        .ok_or("the environment's own values are an object")?;
    let Some(raw) = values.get_mut(KEY) else {
        return Ok(!old.is_empty());
    };
    let requested = raw
        .as_object()
        .ok_or("version overrides must be an object")?;
    let mut prepared = Map::new();
    for (kind, input) in requested {
        let entry = input
            .as_object()
            .ok_or("each version override must be an object")?;
        if entry
            .keys()
            .any(|key| !matches!(key.as_str(), "path" | "version" | "by" | "at"))
        {
            return Err("a version override has an unknown member");
        }
        let path = entry
            .get("path")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("a version override needs its confirmed path")?;
        let version = entry
            .get("version")
            .and_then(Json::as_str)
            .filter(|s| !s.is_empty())
            .ok_or("a version override needs its reported version")?;
        if let Some(previous) = old.get(kind).filter(|previous| {
            previous.get("path").and_then(Json::as_str) == Some(path)
                && previous.get("version").and_then(Json::as_str) == Some(version)
        }) {
            prepared.insert(kind.clone(), previous.clone());
            continue;
        }
        if !confirmed(machine_facts).iter().any(|program| {
            program.kind == *kind && program.path == path && program.version == version
        }) {
            return Err(
                "the machine's confirmed program or its version changed; refresh and try again",
            );
        }
        prepared.insert(
            kind.clone(),
            json!({"path":path,"version":version,"by":by,"at":at}),
        );
    }
    let changed = prepared != old;
    *raw = Json::Object(prepared);
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    fn facts(path: &str, version: &str) -> Json {
        json!({"policy":{"agent.harnesses":[{"kind":"codex","path":path}]},
            "harnesses":[{"kind":"codex","path":path,"version":version,"daemon":false}]})
    }

    #[test]
    fn a_manager_approves_only_the_reported_program_and_the_server_stamps_its_writer() {
        let writer = new_id();
        let attacker = new_id();
        let now: Time = "2026-10-31T00:00:00.000Z".parse().unwrap();
        let mut requested = json!({"exec.version_overrides":{"codex":{"path":"/codex","version":"0.164.0","by":attacker,"at":"2000-01-01T00:00:00.000Z"}}});
        assert!(
            prepare(
                &mut requested,
                &json!({}),
                &facts("/codex", "0.164.0"),
                writer,
                now
            )
            .unwrap()
        );
        assert_eq!(requested[KEY]["codex"]["by"], json!(writer));
        assert_eq!(requested[KEY]["codex"]["at"], json!(now));
        let before = requested.clone();
        requested[KEY]["codex"]["by"] = json!(attacker);
        assert!(
            !prepare(
                &mut requested,
                &before,
                &facts("/codex", "0.164.0"),
                attacker,
                now
            )
            .unwrap()
        );
        assert_eq!(
            requested, before,
            "a second writer cannot forge who approved it"
        );
        let mut stale =
            json!({"exec.version_overrides":{"codex":{"path":"/codex","version":"0.164.0"}}});
        assert!(
            prepare(
                &mut stale,
                &json!({}),
                &facts("/codex", "0.165.0"),
                writer,
                now
            )
            .is_err()
        );
        let mut other =
            json!({"exec.version_overrides":{"codex":{"path":"/other","version":"0.164.0"}}});
        assert!(
            prepare(
                &mut other,
                &json!({}),
                &facts("/codex", "0.164.0"),
                writer,
                now
            )
            .is_err()
        );
        let mut removed = json!({"exec.version_overrides":{}});
        assert!(
            prepare(
                &mut removed,
                &before,
                &facts("/codex", "0.164.0"),
                writer,
                now
            )
            .unwrap()
        );
        assert_eq!(removed[KEY], json!({}));
    }

    #[test]
    fn a_missing_historical_writer_does_not_revoke_a_current_path_and_version() {
        let writer = new_id();
        let at: Time = "2026-10-31T00:00:00.000Z".parse().unwrap();
        let config =
            json!({(KEY):{"codex":{"path":"/codex","version":"unknown","by":writer,"at":at}}});
        let programs = view(&config, &facts("/codex", "unknown"), &Default::default());
        let approval = programs[0].approval.as_ref().unwrap();
        assert_eq!(approval.by.id, writer);
        assert!(approval.by.display_name.is_empty());
        assert!(approval.by.username.is_empty());
        assert!(approval.by.address.is_none());
        assert_eq!(approval.at, at);
        assert!(
            view(
                &config,
                &facts("/replacement", "unknown"),
                &Default::default()
            )[0]
            .approval
            .is_none(),
            "the same version at another path still needs a new decision"
        );
    }
}
