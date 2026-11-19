//! Current stored workspace identity, without exposing private source names or locations.

use crate::middleware::requests::{ApiError, ApiResult};
use orch_db::{queries::environments::routes::EnvironmentFull, secrets::MasterKey};
use orch_driver_contract::runtime::{EnvironmentDataSource, EnvironmentDataSources};
use orch_settings_contract::registry::keys::ENV_DATA_SOURCES;
use orch_types::Id;
use serde::Serialize;

#[derive(Serialize)]
struct Source<'a> {
    purpose: &'static str,
    environment: Id,
    placement: &'a str,
    generation: i32,
    machine: Option<Id>,
    root: &'a str,
    workspace: Option<EnvironmentDataSource>,
}

/// Every source member comes from the same already-read persisted row. In particular the command
/// target is not a later public physical-policy projection or a planned successor's host.
pub(super) fn of(key: &MasterKey, row: &EnvironmentFull) -> ApiResult<String> {
    let selected = row
        .config
        .get(ENV_DATA_SOURCES)
        .cloned()
        .map(serde_json::from_value::<EnvironmentDataSources>)
        .transpose()
        .map_err(ApiError::internal)?;
    let source = Source {
        purpose: "environment/workspace-incarnation/v1",
        environment: row.id,
        placement: &row.placement,
        generation: row.generation,
        machine: row.machine_id,
        root: &row.workspace_root,
        workspace: selected.map(|value| value.workspace),
    };
    let bytes = serde_json::to_vec(&source).map_err(ApiError::internal)?;
    let hash = key.fingerprint(&bytes).map_err(ApiError::internal)?;
    Ok(format!("wsi1_{hash}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::{Json, new_id};
    use serde_json::json;

    fn row() -> EnvironmentFull {
        EnvironmentFull {
            id: new_id(),
            name: "Environment".into(),
            owner_id: new_id(),
            org_id: new_id(),
            placement: "connected".into(),
            home_node_id: new_id(),
            workspace_root: "/srv/workspace".into(),
            config: json!({}),
            machine_id: Some(new_id()),
            template_id: None,
            generation: 1,
            remote_write: false,
            terminal: false,
            browser: false,
            vnc: false,
            last_activity_at: None,
            granted_capabilities: Vec::new(),
            status: "running".into(),
            status_detail: None,
            facts: json!({}),
            manifest_at: None,
            env_seq: 1,
            unused_since: None,
            remove_at: None,
            revision: 1,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn source_members_and_deployment_key_change_the_opaque_value_without_revealing_inputs()
    -> ApiResult<()> {
        let key = MasterKey::new([7; 32]);
        let original = row();
        let token = of(&key, &original)?;
        assert_eq!(of(&key, &original)?, token);
        assert_ne!(of(&MasterKey::new([8; 32]), &original)?, token);
        assert_eq!(token.len(), 5 + 64);
        assert!(token.strip_prefix("wsi1_").is_some_and(|tail| {
            tail.bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }));
        assert!(!token.contains(&original.workspace_root));
        if let Some(machine) = original.machine_id {
            assert!(!token.contains(&machine.to_string()));
        }
        for change in 0..6 {
            let mut next = original.clone();
            match change {
                0 => next.id = new_id(),
                1 => next.generation += 1,
                2 => next.machine_id = Some(new_id()),
                3 => next.workspace_root = "/srv/replaced".into(),
                4 => next.placement = "managed".into(),
                _ => {
                    next.config = json!({(ENV_DATA_SOURCES):{"workspace":{"kind":"existing_directory","path":"/srv/selected"}}})
                }
            }
            assert_ne!(of(&key, &next)?, token);
        }
        Ok(())
    }

    #[test]
    fn unrelated_metadata_and_home_changes_preserve_the_same_source_token() -> ApiResult<()> {
        let key = MasterKey::new([7; 32]);
        let mut original = row();
        original.config = json!({(ENV_DATA_SOURCES):{"workspace":{"kind":"existing_directory","path":"/srv/workspace"},"home":{"kind":"made_by_orch"}}});
        let token = of(&key, &original)?;
        let mut next = original.clone();
        next.name = "Renamed".into();
        next.owner_id = new_id();
        next.org_id = new_id();
        next.home_node_id = new_id();
        next.status = "stopped".into();
        next.status_detail = Some("Offline".into());
        next.env_seq += 1;
        next.revision += 1;
        next.facts = json!({"online":false,"policy":{"changed":true}});
        next.last_activity_at = Some(chrono::Utc::now());
        next.manifest_at = Some(chrono::Utc::now());
        next.remote_write = true;
        next.granted_capabilities.push("changed".into());
        next.config[ENV_DATA_SOURCES]["home"] =
            json!({"kind":"existing_directory","path":"/private/home"});
        next.config["env.idle_stop_minutes"] = json!(30);
        assert_eq!(of(&key, &next)?, token);
        Ok(())
    }

    #[test]
    fn an_absent_legacy_selection_remains_distinct_from_an_explicit_empty_selection()
    -> ApiResult<()> {
        let key = MasterKey::new([7; 32]);
        let mut value = row();
        let absent = of(&key, &value)?;
        value.config[ENV_DATA_SOURCES] = json!({});
        let explicit = of(&key, &value)?;
        assert_ne!(absent, explicit);
        value.config[ENV_DATA_SOURCES] =
            json!({"workspace":{"kind":"made_by_orch"},"home":{"kind":"made_by_orch"}});
        assert_eq!(of(&key, &value)?, explicit);
        value.config[ENV_DATA_SOURCES] = Json::String("invalid".into());
        assert!(
            of(&key, &value).is_err(),
            "an invalid source cannot fabricate an incarnation"
        );
        Ok(())
    }
}
