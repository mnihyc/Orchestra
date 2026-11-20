//! Durable preparation retries never consume or replace the original input.
use super::*;

impl Engine {
    /// One fast retry precedes a blocking state; later attempts use retained bounded backoff.
    pub(super) async fn retain_provider_retry(
        &self,
        unit: &mut Unit<'_>,
        snap: &StartSnapshot,
        message: &str,
    ) -> Result<Option<Blocked>, EngineError> {
        let attempts:i64 = sqlx::query_scalar("INSERT INTO thread_start_retries(input_id,thread_id,provider_id,execution_revision,attempts,retry_at,message)
            VALUES($1,$2,$5,$6,1,$3,$4) ON CONFLICT(input_id) DO UPDATE
            SET attempts=CASE WHEN thread_start_retries.provider_id=EXCLUDED.provider_id
                 AND thread_start_retries.execution_revision=EXCLUDED.execution_revision
                 THEN thread_start_retries.attempts+1 ELSE 1 END,
                provider_id=EXCLUDED.provider_id,execution_revision=EXCLUDED.execution_revision,
                message=EXCLUDED.message RETURNING attempts")
            .bind(snap.input.id).bind(snap.thread.node_id).bind(unit.now()).bind(message)
            .bind(snap.provider.id).bind(snap.thread.execution_revision)
            .fetch_one(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        let seconds = if attempts == 1 {
            0
        } else {
            let base = orch_settings::get::<u64>(
                unit.conn,
                orch_settings::Place::Node(snap.thread.node_id),
                orch_settings_contract::registry::keys::THREADS_START_RETRY_SECS,
            )
            .await?
            .ok_or_else(|| EngineError::unreadable("the preparation retry delay is absent"))?;
            let maximum = orch_settings::get::<u64>(
                unit.conn,
                orch_settings::Place::Node(snap.thread.node_id),
                orch_settings_contract::registry::keys::THREADS_START_RETRY_MAX_SECS,
            )
            .await?
            .ok_or_else(|| EngineError::unreadable("the preparation retry maximum is absent"))?;
            let exponent = u32::try_from(attempts.saturating_sub(2)).unwrap_or(u32::MAX);
            base.saturating_mul(2_u64.saturating_pow(exponent))
                .min(maximum)
        };
        let delay = chrono::Duration::from_std(Duration::from_secs(seconds)).map_err(|_| {
            EngineError::unreadable("the preparation retry delay exceeds the clock's range")
        })?;
        let at = unit.now().checked_add_signed(delay).ok_or_else(|| {
            EngineError::unreadable("the preparation retry date exceeds the clock's range")
        })?;
        sqlx::query("UPDATE thread_start_retries SET retry_at=$2 WHERE input_id=$1")
            .bind(snap.input.id)
            .bind(at)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        Ok((attempts > 1).then(|| {
            Blocked::new(
                "provider_unavailable",
                format!("{message} Retrying automatically."),
            )
        }))
    }

    /// This is checked after input permission and binding checks, before reserving another turn.
    pub(crate) async fn provider_retry_wait(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        input: &InputRow,
    ) -> Result<Option<Blocked>, EngineError> {
        if thread.state != thread_state::BLOCKED {
            return Ok(None);
        }
        let message: Option<String> = sqlx::query_scalar(
            "SELECT message FROM thread_start_retries
            WHERE input_id=$1 AND thread_id=$2 AND retry_at>$3
            AND provider_id=$4 AND execution_revision=$5",
        )
        .bind(input.id)
        .bind(input.thread_id)
        .bind(unit.now())
        .bind(thread.provider_id)
        .bind(thread.execution_revision)
        .fetch_optional(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        Ok(message.map(|message| {
            Blocked::new(
                "provider_unavailable",
                format!("{message} Retrying automatically."),
            )
        }))
    }

    pub(crate) async fn clear_provider_retry(
        &self,
        unit: &mut Unit<'_>,
        input: Id,
    ) -> Result<(), EngineError> {
        sqlx::query("DELETE FROM thread_start_retries WHERE input_id=$1")
            .bind(input)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        Ok(())
    }
}
