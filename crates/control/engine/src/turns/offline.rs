//! The current turn's durable outage, inside existing presence, supervision and forced-stop paths.

use super::*;

impl Engine {
    async fn turn_machine_in(
        &self,
        conn: &mut orch_db::Conn,
        thread: Id,
        turn: &TurnRow,
    ) -> Result<Option<Id>, EngineError> {
        let recorded: Option<Id> =
            sqlx::query_scalar("SELECT machine_id FROM segments WHERE id=$1")
                .bind(turn.segment_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?
                .flatten();
        match recorded {
            Some(machine) => Ok(Some(machine)),
            None => self.harness_machine_of(conn, thread).await,
        }
    }
    /// Capture the effective inherited/lockable setting once per actual native outage.
    pub(super) async fn remember_offline_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
    ) -> Result<bool, EngineError> {
        let Some(turn) = threads::active_turn(unit.conn, thread.node_id).await? else {
            return Ok(false);
        };
        let Some(machine) = self
            .turn_machine_in(unit.conn, thread.node_id, &turn)
            .await?
        else {
            return Ok(false);
        };
        let online: Option<bool> = sqlx::query_scalar("SELECT online FROM machines WHERE id=$1")
            .bind(machine)
            .fetch_optional(&mut *unit.conn)
            .await
            .map_err(orch_db::DbError::from)?;
        if online == Some(true) {
            return Ok(false);
        }
        if threads::offline::episode(unit.conn, turn.id)
            .await?
            .is_some_and(|episode| episode.machine == machine)
        {
            return Ok(false);
        }
        let after = orch_settings::get::<Option<i64>>(
            unit.conn,
            orch_settings::Place::Node(thread.node_id),
            orch_settings_contract::registry::keys::THREADS_OFFLINE_DEADLINE,
        )
        .await?
        .flatten();
        let at = unit.now();
        Ok(threads::offline::begin(unit.conn, turn.id, machine, at, after).await?)
    }
    /// Recheck the exact active turn/source and current machine presence under the stop transaction.
    pub(super) async fn offline_deadline_due_in(
        &self,
        unit: &mut Unit<'_>,
        thread: &ThreadRow,
        turn: &TurnRow,
    ) -> Result<Option<Id>, EngineError> {
        if threads::segment(unit.conn, turn.segment_id)
            .await?
            .is_none_or(|segment| segment.continuation_id != thread.active_continuation_id)
        {
            return Ok(None);
        }
        if thread.current_turn_id != Some(turn.id) || !turn.is_active() {
            return Ok(None);
        }
        let Some(episode) = threads::offline::episode(unit.conn, turn.id).await? else {
            return Ok(None);
        };
        if !episode.expired(unit.now())
            || self
                .turn_machine_in(unit.conn, thread.node_id, turn)
                .await?
                != Some(episode.machine)
        {
            return Ok(None);
        }
        let online: Option<bool> =
            sqlx::query_scalar("SELECT online FROM machines WHERE id=$1 FOR SHARE")
                .bind(episode.machine)
                .fetch_optional(&mut *unit.conn)
                .await
                .map_err(orch_db::DbError::from)?;
        Ok(
            (online != Some(true) && episode.expired(self.inner.clock.now()))
                .then_some(episode.machine),
        )
    }
    /// Existing reconciliation owns the stop and its uncertain native-work boundary.
    pub(super) async fn drive_offline_deadline(&self, thread: Id) -> Result<(), EngineError> {
        let due = {
            let mut conn = self.db().acquire().await?;
            match threads::active_turn(&mut conn, thread).await? {
                Some(turn) => threads::offline::episode(&mut conn, turn.id)
                    .await?
                    .filter(|episode| episode.expired(self.inner.clock.now()))
                    .map(|_| turn),
                None => None,
            }
        };
        if let Some(turn) = due {
            self.restore_after_offline(thread).await?;
            self.end_interrupted(thread, turn.id, Some("machine_offline"))
                .await?;
        }
        Ok(())
    }
}
