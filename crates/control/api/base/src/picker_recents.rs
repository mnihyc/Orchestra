//! Private shared picker preferences. Callers first establish their current account authority.
use crate::middleware::requests::{ApiError, ApiResult};
use orch_api_contract::personal::MePickerRecents;
use orch_db::{Conn, queries::identity};
use orch_types::Id;
use std::collections::BTreeMap;

/// The actual configured length, shared by every native picker and its preference writer.
pub async fn count(conn: &mut Conn) -> ApiResult<u64> {
    orch_settings::get(
        conn,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::UI_PICKER_RECENT_COUNT,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(|| ApiError::internal("the recent picker length has no value"))
}

/// Reducing the current setting takes effect on the very next read.
pub fn trim(count: u64, mut choices: BTreeMap<String, Vec<String>>) -> MePickerRecents {
    let length = usize::try_from(count).unwrap_or(usize::MAX);
    for keys in choices.values_mut() {
        keys.truncate(length);
    }
    choices.retain(|_, keys| !keys.is_empty());
    MePickerRecents { count, choices }
}

/// One preference batch. Keys alone confer no authority to read the identities they name.
pub async fn read(conn: &mut Conn, user: Id) -> ApiResult<MePickerRecents> {
    let count = count(conn).await?;
    let state = identity::interface_state(conn, user).await?;
    let choices = state
        .into_iter()
        .find(|(key, _)| key == "picker_recents")
        .map(|(_, value)| serde_json::from_value(value))
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    Ok(trim(count, choices))
}
