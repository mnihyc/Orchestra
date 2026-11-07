//! One native record observation retained through its bounded, once-only store read.

use super::*;

pub(crate) struct RecordObservation {
    request: Preparation,
    opened: continuing::preparation::Opened<Phase<Adopting>>,
    until: Option<Time>,
}
pub(crate) struct RecordDeadline {
    caller: Caller,
    until: Option<Time>,
}
impl RecordDeadline {
    pub(crate) fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        if self.caller.key_deadline_allows(clock)
            && self
                .until
                .is_none_or(|until| clock.time().get() < until.get())
        {
            Ok(())
        } else {
            Err(EngineError::BeforeEffect(Box::new(PreparationEnded)))
        }
    }
}
impl RecordObservation {
    pub(crate) async fn open(
        engine: &crate::Engine,
        request: &Preparation,
        key: HarnessKey,
    ) -> Result<Self, EngineError> {
        if request.purpose != Purpose::Adopt {
            return Err(EngineError::BeforeEffect(Box::new(PreparationEnded)));
        }
        let request = request.clone().without_transport();
        let mut tx = engine.inner.db.begin().await?;
        let (caller, until) = request
            .current(&mut tx, engine.inner.clock.as_ref())
            .await
            .map_err(engine_error)?;
        let frame = continuing::preparation::prepare_with_caller::<Phase<Adopting>>(
            &mut tx,
            engine.inner.clock.as_ref(),
            &request.origin,
            &caller,
            Input {
                request: request.clone(),
                key,
                command: None,
            },
        )
        .await
        .map_err(engine_error)?;
        let parts = frame.into_parts();
        let deadline = RecordDeadline {
            caller: parts.principal.caller().clone(),
            until,
        };
        parts
            .first_effect
            .check(engine.inner.clock.as_ref())
            .map_err(engine_error)?;
        deadline.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(DbError::from)?;
        deadline.check(engine.inner.clock.as_ref())?;
        Ok(Self {
            request,
            opened: parts.opened,
            until,
        })
    }

    /// Recheck the same original/native association after IO, retaining every earlier ceiling.
    pub(crate) async fn refresh(
        &mut self,
        engine: &crate::Engine,
    ) -> Result<RecordDeadline, EngineError> {
        let mut tx = engine.inner.db.begin().await?;
        let (caller, until) = self
            .request
            .current(&mut tx, engine.inner.clock.as_ref())
            .await
            .map_err(engine_error)?;
        let frame = self
            .opened
            .refresh_with_caller(&mut tx, engine.inner.clock.as_ref(), &caller)
            .await
            .map_err(engine_error)?;
        let parts = frame.into_parts();
        let until = minimum(self.until, until);
        let deadline = RecordDeadline {
            caller: parts.principal.caller().clone(),
            until,
        };
        parts
            .first_effect
            .check(engine.inner.clock.as_ref())
            .map_err(engine_error)?;
        deadline.check(engine.inner.clock.as_ref())?;
        tx.commit().await.map_err(DbError::from)?;
        deadline.check(engine.inner.clock.as_ref())?;
        self.opened = parts.opened;
        self.until = until;
        Ok(deadline)
    }

    /// The same record source on the actual creator transaction before its rollbackable prefix.
    pub(crate) async fn check_in(
        &self,
        conn: &mut Conn,
        caller: &Caller,
        clock: &dyn Clock,
    ) -> Result<Checkpoint, EngineError> {
        let (raw, until) = self
            .request
            .current(conn, clock)
            .await
            .map_err(engine_error)?;
        let current = caller
            .intersect_restrictions(&raw)
            .map_err(EngineError::unreadable)?;
        let frame = self
            .opened
            .refresh_with_caller(conn, clock, &current)
            .await
            .map_err(engine_error)?;
        let parts = frame.into_parts();
        Ok(Checkpoint {
            caller: parts.principal.caller().clone(),
            until: minimum(self.until, until),
            first: parts.first_effect,
            transport: None,
        })
    }
}
