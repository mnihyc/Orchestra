//! Native schedules retain the starter's actual accepted input boundaries, like goal continuations.
use super::*;

impl Engine {
    /// Retain actual accepted starter provenance, never reconstructing a credential from an id.
    /// A collaborator's turn may use an earlier admitted starter input; that origin remains
    /// independently constrained by its accepted/current keys and ordinary holdings.
    pub async fn capture_agent_schedule_authority(
        &self,
        conn: &mut orch_db::Conn,
        thread: Id,
        source_turn: Id,
        starter: Id,
    ) -> Result<Option<Json>, EngineError> {
        let turn = threads::turn(conn, source_turn)
            .await?
            .filter(|turn| turn.thread_id == thread)
            .ok_or(EngineError::StaleToolSource)?;
        let source = threads::input(conn, turn.input_id)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let input = if source.answers() == Some(starter) {
            source
        } else {
            let id:Option<Id>=sqlx::query_scalar("SELECT i.id FROM thread_inputs i JOIN turns t ON t.input_id=i.id AND t.thread_id=i.thread_id JOIN threads h ON h.node_id=i.thread_id AND h.active_continuation_id=i.continuation_id WHERE i.thread_id=$1 AND COALESCE(i.run_as,i.author_id)=$2 AND t.started_at IS NOT NULL ORDER BY t.ordinal DESC,t.id DESC LIMIT 1")
                .bind(thread).bind(starter).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
            let Some(id) = id else {
                return Ok(None);
            };
            let Some(input) = threads::input(conn, id).await? else {
                return Ok(None);
            };
            input
        };
        let raw = if input.run_as == Some(starter) {
            input.run_as_authority.as_ref()
        } else {
            input.submission_authority.as_ref()
        };
        let Some(authority) = saved_authority(raw)? else {
            return Ok(None);
        };
        Ok(Some(serde_json::to_value(
            authority.with_ordinary_ceiling(starter)?,
        )?))
    }

    /// Validate this actual frozen native delegation through the same input-work producer.
    #[allow(
        clippy::too_many_arguments,
        reason = "one existing input producer retains actual native author and frozen authority"
    )]
    pub async fn validate_agent_scheduled_input(
        &self,
        conn: &mut orch_db::Conn,
        author: Id,
        raw: &Json,
        thread: Id,
        request: &InputRequest,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), InputError> {
        let authority = saved_authority(Some(raw))?.ok_or(InputError::Invalid(
            "the starter's accepted authority is unavailable",
        ))?;
        validate_request(request, None)?;
        let mut unit = Unit::new(conn, now);
        let thread = threads::lock(unit.conn, thread)
            .await?
            .ok_or(InputError::NotFound)?;
        let callers = input_authority_for_request(
            &mut unit,
            &thread,
            request,
            None,
            author,
            &authority,
            self.inner.clock.as_ref(),
        )
        .await?
        .ok_or(InputError::Invalid(
            "the starter's accepted authority no longer permits this future input",
        ))?;
        self.prepare_input_work(
            &mut unit,
            &InputExecution::Origins(&callers),
            &thread,
            request,
            None,
            None,
            None,
        )
        .await?;
        Ok(())
    }

    /// Queue one actual occurrence with all retained input origins intact through first wire.
    #[allow(
        clippy::too_many_arguments,
        reason = "one ordinary scheduled-input producer retains native author and authority"
    )]
    pub async fn submit_agent_scheduled_input(
        &self,
        conn: &mut orch_db::Conn,
        author: Id,
        raw: &Json,
        thread: Id,
        request: &InputRequest,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Taken, InputError> {
        let authority = saved_authority(Some(raw))?.ok_or(InputError::Invalid(
            "the starter's accepted authority is unavailable",
        ))?;
        let mut unit = Unit::new(conn, now);
        let row = threads::lock(unit.conn, thread)
            .await?
            .ok_or(InputError::NotFound)?;
        let callers = input_authority_for_request(
            &mut unit,
            &row,
            request,
            None,
            author,
            &authority,
            self.inner.clock.as_ref(),
        )
        .await?
        .ok_or(InputError::Invalid(
            "the starter's accepted authority no longer permits this future input",
        ))?;
        let mut taken = self
            .submit_with_authority_in(
                &mut unit,
                &callers,
                thread,
                request,
                None,
                Made::AGENT_SCHEDULE,
            )
            .await?;
        let finished = unit.finish().await?;
        if taken.submitted.created {
            taken.submitted.seq = finished
                .touched
                .get(&thread)
                .copied()
                .unwrap_or(taken.submitted.seq);
        }
        Ok(taken)
    }
}
