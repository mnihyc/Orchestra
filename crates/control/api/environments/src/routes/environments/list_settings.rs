//! Resolve static environment display settings in one batch before projecting a resource list.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_db::Conn;

use orch_db::queries::environments::routes::EnvironmentFull;

use orch_db::queries::hierarchy;

use orch_db::queries::settings;

use orch_settings::merge::Link;

use orch_settings::merge::OwnValues;

use orch_settings::merge::Rows;

use orch_settings::merge::Stack;

use orch_settings::merge::resolve_key;

use orch_settings_contract::registry;

use orch_settings_contract::registry::Scope;

use orch_settings_contract::registry::keys;

use orch_types::Json;

use std::collections::HashMap;

use std::collections::HashSet;

pub use orch_api_base::routes::environments::list_settings::DisplaySettings;
/// These two keys have fixed registry defaults; resolving their provenance never calls a machine or another supplier.
pub(super) async fn load(
    conn: &mut Conn,
    environments: &[EnvironmentFull],
) -> ApiResult<DisplaySettings> {
    let mut result = DisplaySettings {
        mounts: HashSet::new(),
        idle_minutes: HashMap::new(),
    };
    if environments.is_empty() {
        return Ok(result);
    }
    let homes: Vec<_> = environments.iter().map(|row| row.home_node_id).collect();
    let nodes = hierarchy::with_ancestors(conn, &homes).await?;
    let nodes: HashMap<_, _> = nodes.into_iter().map(|node| (node.id, node)).collect();
    let keys = [
        keys::ENV_MOUNTS.to_owned(),
        keys::ENV_IDLE_STOP_MINUTES.to_owned(),
    ];
    let ids: Vec<_> = nodes.keys().copied().collect();
    let rows = Rows::new(settings::rows_on_chain(conn, &ids, Some(&keys)).await?);
    let mounts = registry::get(&keys[0])
        .ok_or_else(|| ApiError::internal("mount settings are not registered"))?;
    let idle = registry::get(&keys[1])
        .ok_or_else(|| ApiError::internal("idle-stop settings are not registered"))?;
    for environment in environments {
        let home = nodes
            .get(&environment.home_node_id)
            .ok_or_else(ApiError::not_found)?;
        let chain: Vec<Link> = home
            .path
            .iter()
            .map(|id| {
                let node = nodes.get(id).ok_or_else(ApiError::not_found)?;
                let kind: orch_access_contract::roles::NodeKind =
                    node.kind.parse().map_err(ApiError::internal)?;
                Ok(Link {
                    id: *id,
                    scope: Scope::from(kind),
                })
            })
            .collect::<ApiResult<_>>()?;
        let own = OwnValues {
            scope: Scope::Environment,
            values: environment.config.clone(),
            revision: environment.revision,
        };
        let stack = Stack {
            rows: &rows,
            chain: &chain,
            own: Some(&own),
        };
        if resolve_key(mounts, &stack, None)
            .value
            .as_ref()
            .and_then(Json::as_object)
            .is_some_and(|values| values.values().any(|value| value != &Json::Bool(false)))
        {
            result.mounts.insert(environment.id);
        }
        let minutes = resolve_key(idle, &stack, None)
            .value
            .as_ref()
            .and_then(Json::as_i64)
            .unwrap_or_default();
        result.idle_minutes.insert(environment.id, minutes);
    }
    Ok(result)
}
