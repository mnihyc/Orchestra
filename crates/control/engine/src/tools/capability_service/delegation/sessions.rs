//! Idle private conversations close through the ordinary durable lifecycle controls.
use crate::{engine::Engine, error::EngineError};
use chrono::{DateTime, Utc};
use orch_db::queries::{capability_work, threads};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;

impl Engine {
    /// Release only the retained initial native context; provider process policy remains independent.
    pub(crate) async fn close_idle_program_sessions(
        &self,
        now: DateTime<Utc>,
    ) -> Result<u64, EngineError> {
        let retry = chrono::Duration::from_std(crate::commands::BUSY_RETRY_PAUSE)
            .map_err(EngineError::unreadable)?;
        let retry_before = now - retry;
        let mut after = None;
        let mut closed = 0;
        loop {
            let page = {
                let mut conn = self.db().acquire().await?;
                capability_work::idle_programs(
                    &mut conn,
                    retry_before,
                    after,
                    self.inner.config.sweep_batch,
                )
                .await?
            };
            if page.is_empty() {
                break;
            }
            for candidate in page {
                after = Some(candidate.work);
                let mut tx = self.db().begin().await?;
                if threads::lock(&mut tx, candidate.helper).await?.is_none() {
                    continue;
                }
                let Some(current) =
                    capability_work::idle_program(&mut tx, retry_before, candidate.work).await?
                else {
                    continue;
                };
                let seconds = get::<i64>(
                    &mut tx,
                    Place::Provider(current.provider),
                    keys::DELEGATION_SESSION_IDLE_SECS,
                )
                .await?
                .ok_or_else(|| {
                    EngineError::unreadable("The delegated context idle policy is unavailable.")
                })?;
                if now.signed_duration_since(current.completed).num_seconds() < seconds {
                    continue;
                }
                let previous = current.previous_close.unwrap_or(current.work);
                let command = crate::ids::derived(
                    "capability.context.close",
                    &[
                        current.work.as_bytes(),
                        current.segment.as_bytes(),
                        previous.as_bytes(),
                    ],
                );
                let outgoing = crate::commands::session_close(
                    &mut tx,
                    command,
                    current.machine,
                    current.helper,
                    Some(current.segment),
                )
                .await?;
                let mut unit = crate::unit::Unit::new(&mut tx, now);
                if current.previous_close.is_none() {
                    let seq = unit.stamp(current.helper).await?;
                    super::unit::forget_context(unit.conn, current.segment, seq).await?;
                }
                crate::archive::persist(unit.conn, now, outgoing).await?;
                capability_work::retain_idle_close(unit.conn, current.work, command).await?;
                let finished = unit.finish().await?;
                self.commit(tx, finished).await?;
                closed += 1;
            }
        }
        Ok(closed)
    }
}
