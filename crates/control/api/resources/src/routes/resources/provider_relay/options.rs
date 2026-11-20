//! Captured operating settings for one model exchange.
use super::*;
use orch_settings::Place;
use orch_settings_contract::registry::{self, keys};
use orch_types::Time;

#[derive(Clone, Copy)]
pub(super) struct Options {
    pub concurrency: usize,
    pub request_bytes: u64,
    pub idle_secs: u64,
    pub retry_ms: u64,
    pub until: Time,
}
impl Options {
    pub async fn read(ctx: &Ctx) -> ApiResult<Self> {
        let mut conn = ctx.app.db.acquire().await?;
        let names = [
            keys::PROVIDERS_RELAY_CONCURRENCY,
            keys::PROVIDERS_RELAY_REQUEST_BYTES,
            keys::PROVIDERS_RELAY_IDLE_SECS,
            keys::PROVIDERS_RELAY_RETRY_MS,
            keys::JOB_IO_TIMEOUT_SECS,
        ];
        let definitions = names
            .iter()
            .map(|key| {
                registry::get(key)
                    .ok_or_else(|| ApiError::internal("a relay setting has no definition"))
            })
            .collect::<ApiResult<Vec<_>>>()?;
        let snapshot = orch_settings::Snapshot::load(
            &mut conn,
            Place::System,
            orch_settings::Wanted::Keys(&definitions),
        )
        .await
        .map_err(ApiError::internal)?;
        let get = |key| -> ApiResult<u64> {
            snapshot
                .resolve(
                    registry::get(key)
                        .ok_or_else(|| ApiError::internal("a relay setting has no definition"))?,
                )
                .value
                .and_then(|value| value.as_u64())
                .ok_or_else(|| ApiError::internal("a relay setting has no integer default"))
        };
        let until = i64::try_from(get(keys::JOB_IO_TIMEOUT_SECS)?)
            .ok()
            .and_then(chrono::Duration::try_seconds)
            .and_then(|wait| ctx.app.clock.now().checked_add_signed(wait))
            .unwrap_or(chrono::DateTime::<chrono::Utc>::MAX_UTC);
        Ok(Self {
            concurrency: usize::try_from(get(keys::PROVIDERS_RELAY_CONCURRENCY)?)
                .unwrap_or(usize::MAX),
            request_bytes: get(keys::PROVIDERS_RELAY_REQUEST_BYTES)?,
            idle_secs: get(keys::PROVIDERS_RELAY_IDLE_SECS)?,
            retry_ms: get(keys::PROVIDERS_RELAY_RETRY_MS)?,
            until: Time::new(until),
        })
    }
}
