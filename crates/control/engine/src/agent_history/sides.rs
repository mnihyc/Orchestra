//! Recovered sides keep their original source grants and their own recipient admission.

use super::*;
use crate::sessions::StartSnapshot;
use orch_db::queries::{link, transaction_guard};
use orch_types::Time;

impl Engine {
    /// A durable own recipe distinguishes recovery from the original parent preparation.
    pub(crate) async fn agent_history_side_recipient(
        &self,
        snapshot: &StartSnapshot,
    ) -> Result<bool, EngineError> {
        if snapshot.thread.side_of.is_none() {
            return Ok(false);
        }
        Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM segments WHERE id=$1 AND thread_id=$2 AND agent_history_recovery IS NOT NULL)")
            .bind(snapshot.segment.id).bind(snapshot.thread.node_id)
            .fetch_one(&mut *self.db().acquire().await?).await.map_err(orch_db::DbError::from)?)
    }

    /// Each own side effect retains its exact command under current and original source authority.
    pub(crate) async fn admit_agent_history_side(
        &self,
        snapshot: &StartSnapshot,
        mut command: Outgoing,
    ) -> Result<Outgoing, EngineError> {
        if snapshot.thread.side_of.is_none() {
            return Ok(command);
        }
        let mut tx = self.db().begin().await?;
        transaction_guard::lock_action(&mut tx).await?;
        let side = threads::lock(&mut tx, snapshot.thread.node_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("The side is unavailable."))?;
        let current: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns t JOIN thread_inputs i ON i.id=t.input_id
            WHERE t.id=$1 AND t.thread_id=$2 AND t.segment_id=$3 AND t.state='starting' AND t.input_id=$4
            AND t.continuation_id=$5 AND i.continuation_id=$5 AND i.state='delivering')")
            .bind(snapshot.turn.id).bind(side.node_id).bind(snapshot.segment.id)
            .bind(snapshot.input.id).bind(side.active_continuation_id)
            .fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if !current
            || side.current_turn_id != Some(snapshot.turn.id)
            || side.current_segment_id != Some(snapshot.segment.id)
            || side.execution_revision != snapshot.thread.execution_revision
            || side.deleted_at.is_some()
            || side.archived_at.is_some()
        {
            return Err(EngineError::unreadable(
                "The side's actual execution source changed before native admission.",
            ));
        }
        let recipient = threads::segment(&mut tx, snapshot.segment.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("The side segment is unavailable."))?;
        crate::side_threads::history::authorize_recovery(
            self,
            &mut tx,
            &side,
            &recipient,
            self.inner.clock.now(),
        )
        .await?;
        let old = link::command(&mut tx, command.id).await?;
        let deadline = old.as_ref().map_or_else(
            || {
                Time::new(
                    self.inner.clock.now()
                        + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
                )
            },
            |row| Time::new(row.deadline_at),
        );
        command.deadline = Some(deadline);
        let key = command.key.wire();
        link::insert_command(
            &mut tx,
            &link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: key.as_deref(),
                deadline_at: deadline.get(),
            },
            self.inner.clock.now(),
        )
        .await?;
        let retained = link::command(&mut tx, command.id).await?.ok_or_else(|| {
            EngineError::unreadable("The retained side input command is unavailable.")
        })?;
        if retained.machine_id != command.machine
            || retained.kind != command.kind.as_str()
            || retained.serial_key() != key.as_deref()
            || retained.body != command.body
        {
            return Err(EngineError::unreadable(
                "The side's first input command changed identity.",
            ));
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(command)
    }
}
