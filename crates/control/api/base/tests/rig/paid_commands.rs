//! The scripted transport retains guarded native commands through their existing issue admission, and the actual
//! observed creation commands. Common completion reads the real terminal receipt and remains the only ledger writer.

use super::*;

use orch_engine::commands::BeforeCommand;
fn creation(command: &Outgoing) -> bool {
    orch_api_core::testing::captures_native_creation(command.kind)
}

pub(super) fn needs_journal(command: &Outgoing) -> bool {
    command
        .authority
        .as_ref()
        .is_some_and(|authority| authority.requires_issue_checkpoint())
        || creation(command)
}

fn failed(error: impl std::fmt::Display) -> CommandsError {
    CommandsError::Other(error.to_string())
}

pub(super) async fn journal(
    commands: &ScriptedCommands,
    command: &Outgoing,
    before: Option<BeforeCommand<'_>>,
) -> Result<Time, CommandsError> {
    if command.authority.is_none() && !creation(command) {
        return Err(CommandsError::AuthorityMismatch);
    }
    let key = command.key.wire();
    let private = command
        .authority
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(failed)?;
    let mut tx = commands.db.begin().await.map_err(failed)?;
    orch_db::queries::transaction_guard::lock_action(&mut tx)
        .await
        .map_err(failed)?;
    let existing = link_queries::command_with_authority(&mut tx, command.id)
        .await
        .map_err(failed)?;
    let deadline = command
        .deadline
        .or_else(|| {
            existing
                .as_ref()
                .map(|existing| Time::new(existing.command.deadline_at))
        })
        .unwrap_or_else(|| {
            Time::new(
                commands.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
    let proposed = link_queries::NewCommand {
        id: command.id,
        machine_id: command.machine,
        kind: command.kind.as_str(),
        body: &command.body,
        serial_key: key.as_deref(),
        deadline_at: deadline.get(),
    };
    let mut current = None;
    if let Some(existing) = existing {
        let row = existing.command;
        if row.machine_id != command.machine
            || row.kind != command.kind.as_str()
            || row.body != command.body
            || row.serial_key() != key.as_deref()
            || row.deadline_at != deadline.get()
            || existing.authority.as_ref() != private.as_ref()
        {
            return Err(CommandsError::AuthorityMismatch);
        }
        if let Some(check) = before {
            check().map_err(CommandsError::BeforeEffect)?;
        }
    } else {
        let admitted = if let Some(authority) = &command.authority {
            authority
                .lock_native_source(&mut tx)
                .await
                .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?;
            Box::pin(authority.current_issue_callers(
                &mut tx,
                &proposed,
                commands.clock.as_ref(),
                commands.access.as_ref(),
            ))
            .await
            .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?
            .ok_or(CommandsError::AuthorityMismatch)?
        } else {
            // Ordinary browser/profile work already holds its HTTP action guard.
            // Recording its actual queue identity supplies no substitute native authority.
            Vec::new()
        };
        if let Some(check) = before {
            check().map_err(CommandsError::BeforeEffect)?;
        }
        link_queries::insert_command_with_authority(
            &mut tx,
            &proposed,
            commands.clock.now(),
            private.as_ref(),
        )
        .await
        .map_err(failed)?;
        if admitted.iter().any(|(caller, until)| {
            caller
                .as_ref()
                .is_some_and(|caller| !caller.key_deadline_allows(commands.clock.as_ref()))
                || until.is_some_and(|until| commands.clock.time() >= until)
        }) {
            return Err(CommandsError::AuthorityMismatch);
        }
        current = Some(admitted);
    }
    orch_api_core::testing::observe_native_creation(&mut tx, command.id)
        .await
        .map_err(|error| CommandsError::BeforeEffect(Box::new(error)))?;
    tx.commit().await.map_err(failed)?;
    if current.as_ref().is_some_and(|current| {
        commands.clock.time() >= deadline
            || current.iter().any(|(caller, until)| {
                caller
                    .as_ref()
                    .is_some_and(|caller| !caller.key_deadline_allows(commands.clock.as_ref()))
                    || until.is_some_and(|until| commands.clock.time() >= until)
            })
    }) {
        return Err(CommandsError::AuthorityMismatch);
    }
    Ok(deadline)
}
