//! Idle private conversations close through the ordinary durable lifecycle controls.
use crate::{engine::Engine, error::EngineError};
use chrono::{DateTime, Utc};
use orch_db::queries::{capability_work, threads};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::Id;

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
                self.close_program_context(
                    tx,
                    current.work,
                    current.helper,
                    current.segment,
                    current.machine,
                    current.previous_close,
                )
                .await?;
                closed += 1;
            }
        }
        Ok(closed)
    }

    async fn close_program_context(
        &self,
        mut tx: orch_db::Tx,
        work: Id,
        helper: Id,
        segment: Id,
        machine: Id,
        previous: Option<Id>,
    ) -> Result<(), EngineError> {
        let now = self.inner.clock.now();
        let prior = previous.unwrap_or(work);
        let command = crate::ids::derived(
            "capability.context.close",
            &[work.as_bytes(), segment.as_bytes(), prior.as_bytes()],
        );
        let outgoing =
            crate::commands::session_close(&mut tx, command, machine, helper, Some(segment))
                .await?;
        let mut unit = crate::unit::Unit::new(&mut tx, now);
        if previous.is_none() {
            let seq = unit.stamp(helper).await?;
            super::unit::forget_context(unit.conn, segment, seq).await?;
        }
        crate::archive::persist(unit.conn, now, outgoing).await?;
        capability_work::retain_idle_close(unit.conn, work, command).await?;
        let finished = unit.finish().await?;
        self.commit(tx, finished).await?;
        Ok(())
    }

    /// Erase private program material once the actual model session has closed and accounting has settled.
    pub async fn retire_deleted_ask_helpers(&self) -> Result<(), EngineError> {
        let mut after = None;
        loop {
            let page:Vec<(Id,Id)>=sqlx::query_as(
                "SELECT id,helper_id FROM delegated_program_work WHERE purpose='ask' AND source_ask_call_id IS NULL
                 AND ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT $2")
                .bind(after).bind(self.inner.config.sweep_batch).fetch_all(self.db().pool()).await.map_err(orch_db::DbError::from)?;
            if page.is_empty() {
                return Ok(());
            }
            for (work, helper) in page {
                after = Some(work);
                self.retire_ask_helper(super::ParentLink { work, helper })
                    .await?;
            }
        }
    }

    async fn retire_ask_helper(&self, parent: super::ParentLink) -> Result<(), EngineError> {
        use orch_db::DbError;
        super::run::stop_owned(self, &parent, false).await?;
        self.drive_turn_end_work(parent.helper).await?;
        let mut tx = self.db().begin().await?;
        let thread = threads::lock(&mut tx, parent.helper).await?;
        #[derive(sqlx::FromRow)]
        struct Closing {
            opened: bool,
            closed: bool,
            previous: Option<Id>,
            pending: bool,
        }
        let now = self.inner.clock.now();
        let retry = chrono::Duration::from_std(crate::commands::BUSY_RETRY_PAUSE)
            .map_err(EngineError::unreadable)?;
        let Some(state)=sqlx::query_as::<_,Closing>("SELECT w.session_login_hash IS NOT NULL AS opened,
            w.context_closed_at IS NOT NULL AS closed,w.idle_close_command_id AS previous,
            EXISTS(SELECT 1 FROM commands c WHERE c.id=w.idle_close_command_id AND
                (c.state NOT IN ('failed','expired','cancelled') OR c.updated_at>$3)) AS pending
            FROM delegated_program_work w WHERE w.id=$1 AND w.helper_id=$2 AND w.purpose='ask' AND w.source_ask_call_id IS NULL FOR UPDATE")
            .bind(parent.work).bind(parent.helper).bind(now-retry).fetch_optional(&mut *tx).await.map_err(DbError::from)? else{return Ok(());};
        if let Some(thread) = thread {
            if thread.current_turn_id.is_some()
                || threads::turn_end_work_pending(&mut tx, parent.helper).await?
            {
                return Ok(());
            }
            if state.opened && !state.closed {
                if !state.pending
                    && let Some(segment) = thread.current_segment_id
                    && let Some(machine) = threads::segment(&mut tx, segment)
                        .await?
                        .and_then(|row| row.machine_id)
                {
                    self.close_program_context(
                        tx,
                        parent.work,
                        parent.helper,
                        segment,
                        machine,
                        state.previous,
                    )
                    .await?;
                }
                return Ok(());
            }
            let awaiting_wire:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commands WHERE serial_key=$1
                 AND kind IN ('turn.start','turn.start.fenced','session.open','session.open.fenced') AND state IN ('queued','sent','accepted'))")
                .bind(parent.helper.to_string()).fetch_one(&mut *tx).await.map_err(DbError::from)?;
            if awaiting_wire {
                return Ok(());
            }
            sqlx::query("DELETE FROM nodes WHERE id=$1")
                .bind(parent.helper)
                .execute(&mut *tx)
                .await
                .map_err(DbError::from)?;
        }
        // Issued native model and session payloads have the same retention as their private source.
        sqlx::query("DELETE FROM commands WHERE serial_key=$1 AND kind IN ('turn.start','turn.start.fenced','session.open','session.open.fenced')
                     AND state NOT IN ('queued','sent','accepted')")
            .bind(parent.helper.to_string()).execute(&mut *tx).await.map_err(DbError::from)?;
        sqlx::query("DELETE FROM delegated_program_work WHERE id=$1")
            .bind(parent.work)
            .execute(&mut *tx)
            .await
            .map_err(DbError::from)?;
        tx.commit().await.map_err(DbError::from)?;
        Ok(())
    }
}
