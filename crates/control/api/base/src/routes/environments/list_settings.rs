//! Resolve static environment display settings in one batch before projecting a resource list.

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

use chrono::DateTime;

use chrono::Utc;

pub use orch_db::queries::environments::routes::EnvironmentFull;

use orch_types::Id;

pub use std::collections::HashMap;

pub use std::collections::HashSet;
/// Empty mount settings need no per-environment target/permission resolution; nonempty settings keep the full resolver.
pub struct DisplaySettings {
    pub mounts: HashSet<Id>,
    pub idle_minutes: HashMap<Id, i64>,
}

impl DisplaySettings {
    /// Whether the actual merged map names any enabled mount.
    pub fn has_mounts(&self, id: Id) -> bool {
        self.mounts.contains(&id)
    }

    /// The same static idle deadline as the environment lifecycle uses, from the already read descriptor and row.
    pub fn idle_stop(&self, row: &EnvironmentFull, unit: bool) -> ApiResult<Option<DateTime<Utc>>> {
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
