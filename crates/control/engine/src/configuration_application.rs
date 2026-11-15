//! Successful session setup and native pending comparisons, with no private configuration contents.

use crate::{engine::Engine, error::EngineError, sessions::StartSnapshot, unit::Unit};
use orch_api_contract::threads::{
    ConfigurationApplication, SessionConfigurationKind, SessionReloadPolicy,
};
use orch_db::{Conn, queries::threads};
use orch_types::{Id, Json, Time};

fn reload(snapshot: &StartSnapshot) -> SessionReloadPolicy {
    if snapshot.descriptor.capabilities.session_reload
        == orch_agent_contract::descriptor::SessionReload::Turn
        && snapshot.thread.executor.is_none()
    {
        SessionReloadPolicy::EachTurn
    } else {
        SessionReloadPolicy::OnChange
    }
}

/// Called only inside the ordinary current-thread projection, after its existing authority checks.
pub async fn of_thread(
    conn: &mut Conn,
    thread: Id,
    segment: Id,
) -> Result<Option<ConfigurationApplication>, EngineError> {
    let stored: Option<Json> = sqlx::query_scalar::<_, Option<Json>>(
        "SELECT s.configuration_application FROM threads t JOIN segments s ON s.id=t.current_segment_id WHERE t.node_id=$1 AND t.current_segment_id=$2")
        .bind(thread).bind(segment).fetch_optional(conn).await.map_err(orch_db::DbError::from)?.flatten();
    Ok(stored.and_then(|stored| serde_json::from_value(stored).ok()))
}

impl Engine {
    /// Captures the actual comparison before background work can suppress the reopen marker.
    pub(crate) async fn record_configuration_pending(
        &self,
        snapshot: &StartSnapshot,
        pending: Option<Vec<SessionConfigurationKind>>,
        background: bool,
    ) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let Some(thread) = threads::lock(unit.conn, snapshot.thread.node_id).await? else {
            return Ok(());
        };
        if thread.current_segment_id != Some(snapshot.segment.id)
            || thread.current_turn_id != Some(snapshot.turn.id)
        {
            return Ok(());
        }
        let stored: Option<Json> = sqlx::query_scalar(
            "SELECT configuration_application FROM segments WHERE id=$1 FOR UPDATE",
        )
        .bind(snapshot.segment.id)
        .fetch_one(&mut *unit.conn)
        .await
        .map_err(orch_db::DbError::from)?;
        let previous =
            stored.and_then(|value| serde_json::from_value::<ConfigurationApplication>(value).ok());
        let value = ConfigurationApplication {
            accepted_session_revision: previous
                .as_ref()
                .and_then(|value| value.accepted_session_revision),
            accepted_at: previous.and_then(|value| value.accepted_at),
            deferred_by_background: background
                && pending.as_ref().is_some_and(|kinds| !kinds.is_empty()),
            pending_kinds: pending,
            reload_policy: reload(snapshot),
            observed_at: Time::new(unit.now()),
        };
        sqlx::query("UPDATE segments SET configuration_application=$2 WHERE id=$1")
            .bind(snapshot.segment.id)
            .bind(serde_json::to_value(value)?)
            .execute(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        unit.stamp(thread.node_id).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// The acknowledged body, its private restoration hints and public receipt commit together.
    pub(crate) async fn record_configuration_accepted(
        &self,
        snapshot: &StartSnapshot,
        configuration: &crate::harnesses::SessionConfiguration,
        hints: &crate::harnesses::SessionHints,
    ) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let thread = threads::lock(unit.conn, snapshot.thread.node_id).await?;
        sqlx::query("UPDATE segments SET background_wake=$2,session_configuration=$3,session_hints=$4,
            configuration_application=jsonb_build_object(
                'accepted_session_revision',COALESCE((configuration_application->>'accepted_session_revision')::bigint,0)+1,
                'accepted_at',$5::timestamptz,'pending_kinds','[]'::jsonb,'deferred_by_background',false,
                'reload_policy',$6::text,'observed_at',$5::timestamptz) WHERE id=$1")
            .bind(snapshot.segment.id).bind(configuration.background_wake).bind(serde_json::to_value(configuration)?)
            .bind(serde_json::to_value(hints)?).bind(unit.now())
            .bind(match reload(snapshot) { SessionReloadPolicy::EachTurn => "each_turn", SessionReloadPolicy::OnChange => "on_change" })
            .execute(&mut *unit.conn).await.map_err(orch_db::DbError::from)?;
        if let Some(thread) = thread
            && thread.current_segment_id == Some(snapshot.segment.id)
        {
            unit.stamp(thread.node_id).await?;
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }
}
