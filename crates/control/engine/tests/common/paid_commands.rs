//! Store actual paid summary commands under their real native first-effect authority.
//! The scripted peer supplies only transport/model replies; common completion owns accounting.
use super::*;
use orch_engine::commands::{BeforeCommand, RetainedCommand};

pub(super) fn wrap(
    inner: Arc<dyn Commands>,
    db: Db,
    clock: Arc<dyn Clock>,
    access: Arc<dyn ResourceAccess>,
) -> Arc<dyn Commands> {
    Arc::new(Stored {
        inner,
        db,
        clock,
        access,
    })
}
struct Stored {
    inner: Arc<dyn Commands>,
    db: Db,
    clock: Arc<dyn Clock>,
    access: Arc<dyn ResourceAccess>,
}
fn failed(error: impl std::fmt::Display) -> CommandsError {
    CommandsError::Other(error.to_string())
}

#[async_trait]
impl Commands for Stored {
    async fn issue_prepared(
        &self,
        command: Outgoing,
        before: Option<BeforeCommand<'_>>,
    ) -> Result<(), CommandsError> {
        let paid = command.kind == CommandKind::ProviderComplete
            && command
                .authority
                .as_ref()
                .is_some_and(|authority| authority.paid_service_native().is_some());
        if !paid {
            return self.inner.issue_prepared(command, before).await;
        }
        let authority = command
            .authority
            .as_ref()
            .ok_or(CommandsError::AuthorityMismatch)?;
        let deadline = command.deadline.ok_or(CommandsError::AuthorityMismatch)?;
        let key = command.key.wire();
        let proposed = link_queries::NewCommand {
            id: command.id,
            machine_id: command.machine,
            kind: command.kind.as_str(),
            body: &command.body,
            serial_key: key.as_deref(),
            deadline_at: deadline.get(),
        };
        let private = serde_json::to_value(authority).map_err(failed)?;
        let mut tx = self.db.begin().await.map_err(failed)?;
        orch_db::queries::transaction_guard::lock_action(&mut tx)
            .await
            .map_err(failed)?;
        let existing = link_queries::command_with_authority(&mut tx, command.id)
            .await
            .map_err(failed)?;
        if let Some(existing) = existing {
            let row = existing.command;
            if row.machine_id != command.machine
                || row.kind != command.kind.as_str()
                || row.body != command.body
                || row.serial_key() != key.as_deref()
                || row.deadline_at != deadline.get()
                || existing.authority.as_ref() != Some(&private)
            {
                return Err(CommandsError::AuthorityMismatch);
            }
            if let Some(check) = before {
                check().map_err(CommandsError::BeforeEffect)?;
            }
        } else {
            authority
                .lock_native_source(&mut tx)
                .await
                .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?;
            let current = Box::pin(authority.current_issue_callers(
                &mut tx,
                &proposed,
                self.clock.as_ref(),
                self.access.as_ref(),
            ))
            .await
            .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?
            .ok_or(CommandsError::AuthorityMismatch)?;
            if let Some(check) = before {
                check().map_err(CommandsError::BeforeEffect)?;
            }
            link_queries::insert_command_with_authority(
                &mut tx,
                &proposed,
                self.clock.now(),
                Some(&private),
            )
            .await
            .map_err(failed)?;
            if current.iter().any(|(caller, until)| {
                caller
                    .as_ref()
                    .is_some_and(|caller| !caller.key_deadline_allows(self.clock.as_ref()))
                    || until.is_some_and(|until| self.clock.time() >= until)
            }) {
                return Err(CommandsError::AuthorityMismatch);
            }
        }
        tx.commit().await.map_err(failed)?;
        self.inner.issue_prepared(command, None).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.inner.status(id).await
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.inner.wait(id).await
    }
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        self.inner.retained_body(id).await
    }
    async fn retained_command(&self, id: Id) -> Result<Option<RetainedCommand>, CommandsError> {
        self.inner.retained_command(id).await
    }
    async fn retained_return_source(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::credentials::handover_authority::ReturnSource>, CommandsError>
    {
        self.inner.retained_return_source(id).await
    }
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.inner.observe(command).await
    }
    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.cancel(id).await
    }
    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        self.inner.stopped(id).await
    }
    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        self.inner.hears_behind(machine, stream)
    }
}
