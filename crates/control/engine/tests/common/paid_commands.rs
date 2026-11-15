//! Store paid, replacement and helper-session commands under their real native first-effect authority.
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
        replacements: Mutex::new(std::collections::HashSet::new()),
    })
}
struct Stored {
    inner: Arc<dyn Commands>,
    db: Db,
    clock: Arc<dyn Clock>,
    access: Arc<dyn ResourceAccess>,
    replacements: Mutex<std::collections::HashSet<Id>>,
}
fn failed(error: impl std::fmt::Display) -> CommandsError {
    CommandsError::Other(error.to_string())
}

#[async_trait]
impl Commands for Stored {
    async fn issue_prepared(
        &self,
        mut command: Outgoing,
        before: Option<BeforeCommand<'_>>,
    ) -> Result<(), CommandsError> {
        let paid = command.kind == CommandKind::ProviderComplete
            && command
                .authority
                .as_ref()
                .is_some_and(|authority| authority.paid_service_native().is_some());
        let replacement = matches!(
            command.authority.as_ref(),
            Some(orch_engine::commands::CommandAuthority::CapabilityRecreation(_))
        );
        let helper_session = matches!(
            command.authority.as_ref(),
            Some(orch_engine::commands::CommandAuthority::CapabilitySession(
                _
            ))
        );
        if !paid && !replacement && !helper_session {
            return self.inner.issue_prepared(command, before).await;
        }
        if replacement && command.deadline.is_none() {
            let recorded = {
                let mut conn = self.db.acquire().await.map_err(failed)?;
                link_queries::command(&mut conn, command.id)
                    .await
                    .map_err(failed)?
            };
            command.deadline = Some(recorded.map_or_else(
                || {
                    orch_types::Time::new(
                        self.clock.now()
                            + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
                    )
                },
                |row| orch_types::Time::new(row.deadline_at),
            ));
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
                || !existing.authority.as_ref().is_some_and(|saved| {
                    serde_json::from_value::<orch_engine::commands::CommandAuthority>(saved.clone())
                        .is_ok_and(|saved| saved.same_origin(authority))
                })
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
            // The real writer asks its authority about the row it has just stored, and the wire
            // admission asks again before the command goes out. Both read the row back, so what is
            // asked about is the body as the database keeps it (its object members in the
            // database's own order), never the body as the engine built it.
            if paid {
                let stored = link_queries::command(&mut tx, command.id)
                    .await
                    .map_err(failed)?
                    .ok_or_else(|| {
                        failed("the command that was just stored cannot be read back")
                    })?;
                Box::pin(authority.current_callers_after_issue(
                    &mut tx,
                    &stored,
                    &current,
                    self.clock.as_ref(),
                    self.access.as_ref(),
                ))
                .await
                .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?
                .ok_or(CommandsError::AuthorityMismatch)?;
            }
            if current.iter().any(|(caller, until)| {
                caller
                    .as_ref()
                    .is_some_and(|caller| !caller.key_deadline_allows(self.clock.as_ref()))
                    || until.is_some_and(|until| self.clock.time() >= until)
            }) {
                return Err(CommandsError::AuthorityMismatch);
            }
        }
        if paid {
            authority
                .mark_model_admitted(&mut tx, self.clock.time())
                .await
                .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?;
        }
        tx.commit().await.map_err(failed)?;
        if replacement {
            lock(&self.replacements).insert(command.id);
        }
        self.inner.issue_prepared(command, None).await
    }
    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        let info = self.inner.status(id).await?;
        if let Some(outcome) = info.as_ref().and_then(|info| info.outcome.as_ref()) {
            self.finish_replacement(id, outcome).await?;
        }
        Ok(info)
    }
    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        let outcome = self.inner.wait(id).await?;
        self.finish_replacement(id, &outcome).await?;
        Ok(outcome)
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

impl Stored {
    async fn finish_replacement(&self, id: Id, outcome: &Outcome) -> Result<(), CommandsError> {
        let recorded = { lock(&self.replacements).contains(&id) };
        if !recorded {
            return Ok(());
        }
        let mut conn = self.db.acquire().await.map_err(failed)?;
        let Some(stored) = link_queries::command_with_authority(&mut conn, id)
            .await
            .map_err(failed)?
        else {
            return Err(CommandsError::AuthorityMismatch);
        };
        if stored
            .authority
            .as_ref()
            .and_then(|value| value.get("kind"))
            .and_then(Json::as_str)
            != Some("capability_recreation")
        {
            return Err(CommandsError::AuthorityMismatch);
        }
        let retained = self
            .inner
            .retained_command(id)
            .await?
            .ok_or(CommandsError::AuthorityMismatch)?;
        let row = stored.command;
        if retained.machine != row.machine_id
            || retained.kind != row.kind
            || retained.key.as_deref() != row.serial_key()
            || retained.body != row.body
        {
            return Err(CommandsError::AuthorityMismatch);
        }
        let failure = match outcome {
            Outcome::Failed(error) => serde_json::to_value(error).map_err(failed)?,
            _ => Json::Null,
        };
        let ending = match outcome {
            Outcome::Succeeded(value) => link_queries::Ending::Succeeded(value),
            Outcome::Failed(_) => link_queries::Ending::Failed(&failure),
            Outcome::Expired => link_queries::Ending::Expired,
            Outcome::Cancelled => link_queries::Ending::Cancelled,
        };
        link_queries::finish_command(&mut conn, id, &ending, self.clock.now())
            .await
            .map_err(failed)?;
        Ok(())
    }
}
