//! The scripted transport retains paid summary commands through their native admission guard.
//! Common completion reads the real terminal receipt and remains the only ledger writer.

use super::*;

use orch_engine::commands::BeforeCommand;
pub(super) fn is_paid(command: &Outgoing) -> bool {
    command.kind == CommandKind::ProviderComplete
        && command
            .authority
            .as_ref()
            .is_some_and(|authority| authority.paid_service_native().is_some())
}

fn failed(error: impl std::fmt::Display) -> CommandsError {
    CommandsError::Other(error.to_string())
}

pub(super) async fn journal(
    commands: &ScriptedCommands,
    command: &Outgoing,
    before: Option<BeforeCommand<'_>>,
) -> Result<(), CommandsError> {
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
    let mut tx = commands.db.begin().await.map_err(failed)?;
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
            commands.clock.as_ref(),
            commands.access.as_ref(),
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
            commands.clock.now(),
            Some(&private),
        )
        .await
        .map_err(failed)?;
        if current.iter().any(|(caller, until)| {
            caller
                .as_ref()
                .is_some_and(|caller| !caller.key_deadline_allows(commands.clock.as_ref()))
                || until.is_some_and(|until| commands.clock.time() >= until)
        }) {
            return Err(CommandsError::AuthorityMismatch);
        }
    }
    tx.commit().await.map_err(failed)?;
    Ok(())
}
