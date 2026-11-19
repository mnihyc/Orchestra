//! Current pooled work retains its real input origins and stops independently of direct users.
use super::*;
use orch_db::DbError;
use orch_settings::Place;
use orch_settings_contract::registry::keys;

const CAUSE: &str = "pool_access_changed";
const MESSAGE: &str = "Pool access no longer permits this task. The task stopped.";

impl Engine {
    /// Existing supervision observes the real admitted attempts in bounded pages, without file IO.
    pub(crate) async fn keep_pool_uses(&self) -> Result<(), EngineError> {
        let batch = {
            let mut conn = self.db().acquire().await?;
            orch_settings::get::<i64>(&mut conn, Place::System, keys::POOLS_READ_BATCH_SIZE)
                .await?
                .ok_or_else(|| EngineError::unreadable("the pool read batch default is absent"))?
        };
        let mut after = None;
        loop {
            let rows: Vec<(Id, Id)> = {
                let mut conn = self.db().acquire().await?;
                sqlx::query_as("SELECT t.id,t.thread_id FROM turns t WHERE t.state IN ('starting','running') AND t.interrupted_by IS NULL AND ($1::uuid IS NULL OR t.id>$1) AND EXISTS(SELECT 1 FROM pool_attempt_scopes a WHERE a.attempt_id=t.id) ORDER BY t.id LIMIT $2")
                    .bind(after).bind(batch).fetch_all(&mut *conn).await.map_err(DbError::from)?
            };
            if rows.is_empty() {
                break;
            }
            after = rows.last().map(|row| row.0);
            for (turn, thread) in rows {
                Box::pin(self.check_pool_use(thread, turn)).await?;
            }
        }
        Ok(())
    }

    async fn check_pool_use(&self, thread: Id, turn: Id) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        let mut unit = Unit::new(&mut tx, self.inner.clock.now());
        let Some(thread) = threads::lock(unit.conn, thread).await? else {
            return Ok(());
        };
        let Some(turn) = threads::turn(unit.conn, turn).await?.filter(|turn| {
            turn.thread_id == thread.node_id && turn.is_active() && turn.interrupted_by.is_none()
        }) else {
            return Ok(());
        };
        // Another native continuation keeps its own source and interrupt lifecycle.
        if thread.current_turn_id != Some(turn.id) {
            return Ok(());
        }
        let Some(input) = threads::input(unit.conn, turn.input_id).await? else {
            return Ok(());
        };
        let allowed = match self
            .steer_authorized(&mut unit, &thread, &turn, &input)
            .await
        {
            Ok(allowed) => allowed,
            Err(EngineError::Unreadable(_) | EngineError::BeforeEffect(_)) => false,
            Err(error) => return Err(error),
        };
        if allowed {
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
            return Ok(());
        }
        let responsible = input.run_as.or(input.author_id).unwrap_or(thread.owner_id);
        let seq = unit.stamp(thread.node_id).await?;
        if threads::mark_interrupted_by(unit.conn, turn.id, responsible, seq).await? {
            threads::set_interrupt_cause(unit.conn, turn.id, CAUSE).await?;
            ingest::add_error_item(
                &mut unit,
                thread.node_id,
                turn.segment_id,
                Some(turn.id),
                CAUSE,
                MESSAGE,
                false,
            )
            .await?;
            unit.wake(thread.node_id);
        }
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }
}
