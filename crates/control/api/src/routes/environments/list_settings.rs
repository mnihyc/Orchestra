//! Resolve static environment display settings in one batch before projecting a resource list.

use crate::middleware::requests::{ApiError, ApiResult};
use chrono::{DateTime, Utc};
use orch_db::Conn;
use orch_db::queries::environments::routes::EnvironmentFull;
use orch_db::queries::{hierarchy, settings};
use orch_settings::merge::{Link, OwnValues, Rows, Stack, resolve_key};
use orch_settings_contract::registry::{self, Scope, keys};
use orch_types::{Id, Json};
use std::collections::{HashMap, HashSet};

/// Empty mount settings need no per-environment target/permission resolution; nonempty settings keep the full resolver.
pub(super) struct DisplaySettings {
    mounts: HashSet<Id>,
    idle_minutes: HashMap<Id, i64>,
}

impl DisplaySettings {
    /// Whether the actual merged map names any enabled mount.
    pub(super) fn has_mounts(&self, id: Id) -> bool {
        self.mounts.contains(&id)
    }

    /// The same static idle deadline as the environment lifecycle uses, from the already read descriptor and row.
    pub(super) fn idle_stop(
        &self,
        row: &EnvironmentFull,
        unit: bool,
    ) -> ApiResult<Option<DateTime<Utc>>> {
        let minutes = self.idle_minutes.get(&row.id).copied().unwrap_or_default();
        if !unit || row.status != "running" || minutes <= 0 {
            return Ok(None);
        }
        let span = chrono::Duration::try_minutes(minutes).ok_or_else(|| {
            ApiError::internal("the idle-stop duration is outside the supported range")
        })?;
        let at = row
            .last_activity_at
            .unwrap_or(row.created_at)
            .checked_add_signed(span)
            .ok_or_else(|| {
                ApiError::internal("the idle-stop deadline is outside the supported range")
            })?;
        Ok(Some(at))
    }
}

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
