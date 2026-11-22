//! Operational upstream policy resolved once for the actual provider start.

use crate::{Error, Place, Snapshot, Wanted};
use orch_db::Conn;
use orch_provider_contract::models::UpstreamPolicy;
use orch_settings_contract::registry::{get, keys};

/// Captures the same effective setting values for native and public model requests.
pub async fn load(conn: &mut Conn, place: Place) -> Result<UpstreamPolicy, Error> {
    let names = [
        keys::PROVIDERS_UPSTREAM_HEAD_SECS,
        keys::PROVIDERS_UPSTREAM_IDLE_SECS,
        keys::PROVIDERS_UPSTREAM_WHOLE_BYTES,
    ];
    let definitions = names
        .into_iter()
        .map(|key| get(key).ok_or_else(|| Error::UnknownKey(key.into())))
        .collect::<Result<Vec<_>, _>>()?;
    let snapshot = Snapshot::load(conn, place, Wanted::Keys(&definitions)).await?;
    let value = |index: usize| {
        snapshot
            .resolve(definitions[index])
            .value
            .and_then(|value| value.as_u64())
            .filter(|value| *value > 0)
            .ok_or(Error::Unreadable {
                what: "model upstream policy",
            })
    };
    Ok(UpstreamPolicy {
        head_secs: value(0)?,
        idle_secs: value(1)?,
        whole_bytes: value(2)?,
    })
}
