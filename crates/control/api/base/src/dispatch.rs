//! Durable commands and redelivery.
//!
//! A command is a row before it is anything else. It is sent whenever its
//! machine has a session, again after every reconnect and every restart of
//! the system, until the machine has answered, its deadline has passed
//! before the machine took it, or it was cancelled; the machine runs it once
//! however often it is sent, because it answers one it has seen from what
//! it wrote down. The answer is stored in the row.
//!
//! What the row holds is the command as the system keeps it, and not as it is
//! sent. A secret is a reference to the secret's own row, never its value,
//! and is replaced by the value sealed to the machine the command goes to at
//! the moment the frame is written; a ticket is a request, and is replaced by
//! a ticket signed at that moment. A command's serial key has a protected
//! column. Older retained bodies can still read `$key`, which is removed
//! before the machine request is sent.

use crate::backplane::Link;

use crate::backplane::MachineEntry;

use async_trait::async_trait;

use chrono::Duration as ChronoDuration;

use orch_db::queries::link as queries;

use orch_db::queries::link::CommandRow;

use orch_db::queries::link::Ending;

use orch_db::queries::link::NewCommand;

use orch_db::secrets::MasterKey;

use orch_db::Db;

use orch_db::DbError;

use orch_link::commands::CommandFailure;

use orch_link::commands::Progress;

use orch_link_contract::commands::Cmd;

use orch_link_contract::commands::CommandBody;

use orch_link_contract::commands::CommandError;

use orch_link_contract::commands::CommandKind;

use orch_link_contract::commands::ErrorCode;

use orch_link_contract::commands::Reference;

use orch_link_contract::commands::SerialKey;

use orch_link_contract::commands::TicketWanted;

use orch_link_contract::commands::has_references;

use orch_link_contract::commands::is_transport_rebind;

use orch_link_contract::commands::resolve_reference_values;

use orch_link_contract::commands::resolve_references;

use orch_pki::sealing::RecipientKey;

use orch_pki::sealing::info_for_command;

use orch_pki::sealing::seal;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use orch_types::new_id;

use std::collections::HashMap;

use std::time::Duration;
/// The older stored member read only when no protected serial key was supplied.
pub const KEY_MEMBER: &str = "$key";

fn issuing_deadlines_allow(
    callers: &[(Option<orch_authz::Caller>, Option<Time>)],
    clock: &dyn orch_types::Clock,
) -> bool {
    callers.iter().all(|(caller, until)| {
        caller
            .as_ref()
            .is_none_or(|caller| caller.key_deadline_allows(clock))
            && until.is_none_or(|until| clock.now() < until.get())
    })
}

fn provider_request(kind: CommandKind, body: &Json) -> Result<(Id, String), serde_json::Error> {
    if kind == CommandKind::ProviderEmbed {
        let request: orch_link_contract::commands::ProviderEmbed =
            serde_json::from_value(body.clone())?;
        Ok((request.provider_id, request.model_name))
    } else {
        let request: orch_link_contract::commands::ProviderComplete =
            serde_json::from_value(body.clone())?;
        Ok((request.provider_id, request.model_name))
    }
}

/// How often a caller who waits for a command looks at its row though nobody
/// told it anything: a word that was missed is made up for within this.
const WAIT_RECHECK: Duration = Duration::from_secs(2);

/// The reads of a workspace a person makes through the API (a listing, a
/// description, a search, a repository's state and its changes), whose answer
/// the one who asked reads once: [`Dispatcher::run`] clears it from the row as
/// soon as it has it, so that a listing of thousands of entries is not kept
/// for the day a finished command stays.
const ANSWERED_ONCE: [CommandKind; 5] = [
    CommandKind::FsList,
    CommandKind::FsStat,
    CommandKind::FsSearch,
    CommandKind::GitStatus,
    CommandKind::GitDiff,
];

/// Why a command could not be issued, or waited for.
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    /// A native issuing writer's actual first-effect check refused the queue insertion.
    #[error("the command's current issuing boundary has ended: {0}")]
    BeforeEffect(Box<dyn std::error::Error + Send + Sync>),
    /// The system does not know the machine.
    #[error("the system does not know that machine")]
    UnknownMachine,
    /// The machine was revoked.
    #[error("the machine was revoked")]
    Revoked,
    /// There is no such command.
    #[error("there is no such command")]
    UnknownCommand,
    /// The kind is not one of the catalogue.
    #[error("`{0}` is not a kind of command")]
    UnknownKind(String),
    /// The body could not be written.
    #[error("the body of the command cannot be written: {0}")]
    Body(#[from] serde_json::Error),
    /// The database failed.
    #[error("{0}")]
    Db(#[from] DbError),
    /// A new scoped issuer cannot relabel or adopt another canonical native origin.
    #[error(
        "the retained native command belongs to another origin; its existing work was unchanged"
    )]
    AuthorityMismatch,
    /// The system is stopping.
    #[error("the system is stopping")]
    Stopping,
    /// The machine's record could not be read just now.
    #[error("the machine's record could not be read just now")]
    Unavailable,
}

/// A command that was made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Issued {
    /// Its identifier.
    pub id: Id,
    /// The moment after which it is not to be started.
    pub deadline: Time,
}

/// How a command ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandOutcome {
    /// The machine answered.
    Succeeded(Json),
    /// The machine failed it, or did not answer.
    Failed(CommandError),
    /// Its deadline passed before a machine took it.
    Expired,
    /// It was cancelled.
    Cancelled,
}

impl CommandOutcome {
    fn of(row: &CommandRow) -> Option<Self> {
        match row.state.as_str() {
            // An answer cleared once the one who asked had it is, to whoever reads it later, an answer
            // that is no longer to be had: it expired.
            "succeeded" => Some(row.result.clone().map_or(Self::Expired, Self::Succeeded)),
            "failed" => Some(Self::Failed(
                row.error
                    .clone()
                    .and_then(|error| serde_json::from_value(error).ok())
                    .unwrap_or_else(|| {
                        CommandError::new(ErrorCode::Internal, "the command failed")
                    }),
            )),
            "expired" => Some(Self::Expired),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// Why a command that was run did not give its result.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// It could not be issued or waited for.
    #[error("{0}")]
    Dispatch(#[from] DispatchError),
    /// It ended without a result.
    #[error("{0}")]
    Command(#[from] CommandFailure),
}

/// A command as the system keeps it, to be issued.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredCommand {
    /// Its identifier; one is made when none is given, and a command asked
    /// for twice under one identifier is made once.
    pub id: Option<Id>,
    /// The machine it is for.
    pub machine: Id,
    /// Its kind, one of the catalogue.
    pub kind: String,
    /// Its serial key.
    pub key: Option<String>,
    /// The moment after which it is not to be started; the kind's own time
    /// after now when none is given.
    pub deadline: Option<Time>,
    /// Its body, with references in place of secrets and tickets.
    pub body: Json,
    /// Private typed evidence, redacted by its Debug implementation and never copied to Cmd.
    pub authority: Option<orch_engine::commands::CommandAuthority>,
}

/// The command a row holds, with its authoritative key and no legacy storage member in the body.
#[must_use]
pub fn cmd_of(row: &CommandRow) -> Cmd {
    let mut body = row.body.clone();
    let key = row.serial_key().map(str::to_owned);
    if let Some(members) = body.as_object_mut() {
        members.remove(KEY_MEMBER);
    }
    Cmd {
        id: row.id,
        kind: row.kind.clone(),
        key,
        deadline: Time::new(row.deadline_at),
        body,
    }
}

#[cfg(any(test, feature = "test-support"))]
fn stored_body(mut body: Json, key: Option<&str>) -> Json {
    if let (Some(key), Some(members)) = (key, body.as_object_mut()) {
        members.insert(KEY_MEMBER.to_owned(), Json::String(key.to_owned()));
    }
    body
}

/// A mutable accepted HTTP minimum never replaces immutable native provenance.
fn same_private_origin(
    saved: Option<&Json>,
    proposed: Option<&orch_engine::commands::CommandAuthority>,
) -> bool {
    match (saved, proposed) {
        (None, None) => true,
        (Some(saved), Some(proposed)) => {
            serde_json::from_value::<orch_engine::commands::CommandAuthority>(saved.clone())
                .is_ok_and(|saved| saved.same_origin(proposed))
        }
        _ => false,
    }
}

/// What the rest of the API issues commands with.
#[derive(Clone)]
pub struct Dispatcher {
    link: Link,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher").finish_non_exhaustive()
    }
}

impl Dispatcher {
    /// Replaces only an open session's forwarded transport, without storing its new ticket.
    pub async fn observe_transport(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionOpen,
    ) -> Result<Json, DispatchError> {
        if !body.transport_only
            || !matches!(
                &body.provider,
                orch_link_contract::commands::SessionOpenProvider::Forwarded { .. }
            )
        {
            return Err(DispatchError::UnknownKind(
                "only a forwarded session transport rebind uses this transient call".to_owned(),
            ));
        }
        let rebound = self
            .link
            .session_transport(machine, body)
            .await
            .map_err(|_| DispatchError::Unavailable)?;
        serde_json::to_value(rebound).map_err(DispatchError::Body)
    }

    /// Reads only live native session facts, bypassing durable command storage.
    pub async fn observe_background(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionBackground,
    ) -> Result<Json, DispatchError> {
        let observation = self
            .link
            .session_background(machine, body)
            .await
            .map_err(|_| DispatchError::Unavailable)?;
        serde_json::to_value(observation).map_err(DispatchError::Body)
    }

    /// Reads one open session's native catalogue without durable command/result storage.
    pub async fn observe_native_actions(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionNativeActions,
    ) -> Result<Json, DispatchError> {
        let catalog = self
            .link
            .session_native_actions(machine, body)
            .await
            .map_err(|_| DispatchError::Unavailable)?;
        serde_json::to_value(catalog).map_err(DispatchError::Body)
    }

    /// Has the harness of an open session count what fills its context, live, bypassing durable command
    /// storage.
    pub async fn observe_context(
        &self,
        machine: Id,
        body: &orch_link_contract::commands::SessionContext,
    ) -> Result<Json, DispatchError> {
        let counted = self
            .link
            .session_context(machine, body)
            .await
            .map_err(|_| DispatchError::Unavailable)?;
        serde_json::to_value(counted).map_err(DispatchError::Body)
    }

    pub fn new(link: Link) -> Self {
        Self { link }
    }

    /// Issues a command of the catalogue, with the time its kind allows.
    pub async fn issue<B: CommandBody>(
        &self,
        machine: Id,
        key: &SerialKey,
        body: &B,
    ) -> Result<Issued, DispatchError> {
        self.issue_stored(StoredCommand {
            authority: None,
            id: None,
            machine,
            kind: B::KIND.as_str().to_owned(),
            key: key.wire(),
            deadline: None,
            body: serde_json::to_value(body)?,
        })
        .await
    }

    /// Issues a command of the catalogue under an identifier of the caller's
    /// and with a deadline of its own.
    pub async fn issue_as<B: CommandBody>(
        &self,
        id: Id,
        machine: Id,
        key: &SerialKey,
        deadline: Time,
        body: &B,
    ) -> Result<Issued, DispatchError> {
        self.issue_stored(StoredCommand {
            authority: None,
            id: Some(id),
            machine,
            kind: B::KIND.as_str().to_owned(),
            key: key.wire(),
            deadline: Some(deadline),
            body: serde_json::to_value(body)?,
        })
        .await
    }

    /// Stores a command and hands it to the machine's link: it is sent at
    /// once if the machine has a session, and whenever it has one until the
    /// command is over.
    pub async fn issue_stored(&self, command: StoredCommand) -> Result<Issued, DispatchError> {
        self.issue_stored_boundary(command, None).await
    }

    /// Queue the same native command with its writer's check at the first SQL mutation.
    pub async fn issue_stored_before(
        &self,
        command: StoredCommand,
        before: orch_engine::commands::BeforeCommand<'_>,
    ) -> Result<Issued, DispatchError> {
        self.issue_stored_boundary(command, Some(before)).await
    }

    fn issue_stored_boundary<'a>(
        &'a self,
        command: StoredCommand,
        before: Option<orch_engine::commands::BeforeCommand<'a>>,
    ) -> futures::future::BoxFuture<'a, Result<Issued, DispatchError>> {
        Box::pin(async move {
            let guarded = before.is_some();
            if command.kind == "session.background" {
                return Err(DispatchError::UnknownKind(
                    "live native reads must use the transient observation path".to_owned(),
                ));
            }
            if is_transport_rebind(&command.kind, &command.body) {
                return Err(DispatchError::UnknownKind(
                    "session transport rebinds must use the transient path".to_owned(),
                ));
            }
            if command
                .authority
                .as_ref()
                .is_some_and(|authority| authority.command_kind() != command.kind)
            {
                return Err(DispatchError::UnknownKind(
                    "the private command evidence belongs to another command".into(),
                ));
            }
            let kind: CommandKind = command
                .kind
                .parse()
                .map_err(|_| DispatchError::UnknownKind(command.kind.clone()))?;
            let entry = self
                .link
                .entry(command.machine)
                .await
                .map_err(|error| match error {
                    crate::backplane::EntryError::Unknown => DispatchError::UnknownMachine,
                    crate::backplane::EntryError::Revoked => DispatchError::Revoked,
                    crate::backplane::EntryError::Database => DispatchError::Unavailable,
                })?;
            let inner = &self.link.inner;
            let now = inner.clock.now();
            let id = command.id.unwrap_or_else(new_id);
            let deadline = command.deadline.map_or_else(
                || now + ChronoDuration::seconds(i64::from(kind.deadline_secs())),
                Time::get,
            );
            let mut body = command.body;
            let mut tx = inner.db.begin().await?;
            let owner_read_result = if kind == CommandKind::OrchToolResult {
                let tools = inner
                    .read_tools
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                match tools {
                    Some(tools) => tools
                        .prepare_result(&mut tx, id, &mut body)
                        .await
                        .map_err(|error| DispatchError::BeforeEffect(Box::new(error)))?,
                    None => false,
                }
            } else {
                false
            };
            if let Some(authority) = &command.authority {
                authority
                    .lock_native_source(&mut tx)
                    .await
                    .map_err(|_| DispatchError::Unavailable)?;
            }
            orch_engine::jobs::provision::lock_creator_command_source(&mut tx, id)
                .await
                .map_err(|error| DispatchError::BeforeEffect(Box::new(error)))?;
            let provider_allowed = if matches!(
                kind,
                CommandKind::ProviderComplete | CommandKind::ProviderEmbed
            ) {
                orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
                let (provider, model) = provider_request(kind, &body)?;
                orch_engine::resource_facts::activity::accepts_command(
                    &mut tx, id, provider, &model,
                )
                .await
                .map_err(|_| DispatchError::Unavailable)?
            } else {
                true
            };
            let work_owner = body
                .pointer("/work/job_id")
                .and_then(Json::as_str)
                .and_then(|value| value.parse::<Id>().ok());
            let parents: Vec<bool> = sqlx::query_scalar("SELECT plan ? 'cancel' OR state NOT IN ('queued','running')
            FROM jobs WHERE id=$2 OR (state IN ('queued','running') AND plan->'active_command'->>'id'=$1::text)
            ORDER BY id FOR UPDATE")
            .bind(id).bind(work_owner).fetch_all(&mut *tx).await.map_err(DbError::from)?;
            let authority = command
                .authority
                .as_ref()
                .map(serde_json::to_value)
                .transpose()?;
            // A new scoped issuer cannot affect an already retained different origin.
            // Keep the same post-insert comparison too for a concurrent first insertion.
            if guarded
                && command
                    .authority
                    .as_ref()
                    .is_some_and(orch_engine::commands::CommandAuthority::is_scoped_native)
                && let Some(saved) = queries::lock_command_with_authority(&mut tx, id).await?
                && (saved.command.machine_id != command.machine
                    || saved.command.kind != kind.as_str()
                    || saved.command.serial_key() != command.key.as_deref()
                    || saved.command.body != body
                    || !same_private_origin(saved.authority.as_ref(), command.authority.as_ref()))
            {
                return Err(DispatchError::AuthorityMismatch);
            }
            // Entry/provider/parent preparation and retained-command comparison can wait.
            // New native queue work keeps its actual original operation on this transaction.
            let issue_callers = if let Some(authority) = command.authority.as_ref()
                && authority.requires_issue_checkpoint()
                && queries::command(&mut tx, id).await?.is_none()
            {
                let current = authority
                    .current_issue_callers(
                        &mut tx,
                        &NewCommand {
                            id,
                            machine_id: command.machine,
                            kind: kind.as_str(),
                            body: &body,
                            serial_key: command.key.as_deref(),
                            deadline_at: deadline,
                        },
                        inner.clock.as_ref(),
                        &crate::thread_access::AuthzAccess::new(inner.clock.clone()),
                    )
                    .await
                    .map_err(|_| DispatchError::Unavailable)?;
                Some(current.ok_or(DispatchError::Unavailable)?)
            } else {
                None
            };
            if let Some(before) = before {
                before().map_err(DispatchError::BeforeEffect)?;
            }
            if issue_callers
                .as_ref()
                .is_some_and(|callers| !issuing_deadlines_allow(callers, inner.clock.as_ref()))
            {
                return Err(DispatchError::Unavailable);
            }
            let inserted = queries::insert_command_with_authority(
                &mut tx,
                &NewCommand {
                    id,
                    machine_id: command.machine,
                    kind: kind.as_str(),
                    body: &body,
                    serial_key: command.key.as_deref(),
                    deadline_at: deadline,
                },
                now,
                authority.as_ref(),
            )
            .await?;
            if inserted && owner_read_result {
                sqlx::query("UPDATE thread_tool_calls SET plan=jsonb_set(plan,'{owner_read,completed}','true') WHERE answer_id=$1")
                    .bind(id).execute(&mut *tx).await.map_err(DbError::from)?;
                // A frozen/copy turn carries this fact with its existing row. No synthetic tool
                // call or permission proof is invented when previously read history is copied.
                let counted=sqlx::query("UPDATE turns u SET owner_read_results=u.owner_read_results+1 FROM thread_tool_calls c WHERE c.answer_id=$1 AND u.id=c.turn_id AND u.thread_id=c.thread_id")
                    .bind(id).execute(&mut *tx).await.map_err(DbError::from)?;
                if counted.rows_affected() != 1 {
                    return Err(DispatchError::Unavailable);
                }
            }
            if command
                .authority
                .as_ref()
                .is_some_and(orch_engine::commands::CommandAuthority::is_scoped_native)
            {
                let saved = queries::lock_command_with_authority(&mut tx, id)
                    .await?
                    .ok_or(DispatchError::UnknownCommand)?;
                // Original canonical None stays None, and its old queued/system work is not cancelled.
                // Accepted work also keeps its original owner; domain result reconciliation is separate.
                if saved.command.machine_id != command.machine
                    || saved.command.kind != kind.as_str()
                    || saved.command.serial_key() != command.key.as_deref()
                    || saved.command.body != body
                    || !same_private_origin(saved.authority.as_ref(), command.authority.as_ref())
                {
                    return Err(DispatchError::AuthorityMismatch);
                }
            }
            if !provider_allowed {
                let error = serde_json::to_value(CommandError::new(
                    ErrorCode::Conflict,
                    "Provider switched off",
                ))?;
                queries::finish_command(&mut tx, id, &Ending::Failed(&error), now).await?;
            }
            if parents.iter().any(|cancelled| *cancelled)
                || (work_owner.is_some() && parents.is_empty())
            {
                queries::finish_command(&mut tx, id, &Ending::Cancelled, now).await?;
                if inserted {
                    sqlx::query("UPDATE commands SET stop_confirmed_at=$2 WHERE id=$1")
                        .bind(id)
                        .bind(now)
                        .execute(&mut *tx)
                        .await
                        .map_err(DbError::from)?;
                }
            }
            let row = queries::command(&mut tx, id)
                .await?
                .ok_or(DispatchError::UnknownCommand)?;
            if (guarded
                || matches!(
                    &command.authority,
                    Some(orch_engine::commands::CommandAuthority::Http(_))
                ))
                && inserted
                && let Some(authority) = &command.authority
            {
                let current = authority
                    .current_callers(
                        &mut tx,
                        &row,
                        inner.clock.as_ref(),
                        &crate::thread_access::AuthzAccess::new(inner.clock.clone()),
                    )
                    .await
                    .map_err(|error| DispatchError::BeforeEffect(Box::new(error)))?;
                let allowed = current
                    .is_some_and(|callers| issuing_deadlines_allow(&callers, inner.clock.as_ref()));
                if !allowed {
                    return Err(DispatchError::BeforeEffect(Box::new(
                        crate::middleware::requests::ApiError::new(
                            orch_api_contract::common::ErrorCode::AuthzDenied,
                            "the native command's issuing authority is no longer current",
                        ),
                    )));
                }
            }

            crate::middleware::idempotency::commands::observe(&mut tx, &row)
                .await
                .map_err(|error| DispatchError::BeforeEffect(Box::new(error)))?;
            // Queue insertion and bookkeeping may wait after the original issuer was checked.
            // Only new uncommitted work is withdrawn; retained commands keep their receipt.
            if inserted
                && issue_callers
                    .as_ref()
                    .is_some_and(|callers| !issuing_deadlines_allow(callers, inner.clock.as_ref()))
            {
                return Err(DispatchError::BeforeEffect(Box::new(
                    crate::middleware::requests::ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the native command's issuing authority is no longer current",
                    ),
                )));
            }
            tx.commit().await.map_err(DbError::from)?;
            // Lifecycle transactions may have committed this command before the dispatcher sees it.
            #[cfg(any(test, feature = "test-support"))]
            crate::backplane::command_admission_tests::adoption_read(
                &self.link,
                id,
                command.machine,
            )
            .await;
            if row.state == "queued" {
                #[cfg(feature = "test-observers")]
                orch_link::test_observers::issued_probe(command.machine, id, &row.kind);
                entry.adopt(cmd_of(&row), Progress::Queued);
            }
            Ok(Issued {
                id,
                deadline: Time::new(row.deadline_at),
            })
        })
    }

    /// Waits until a command is over, and says how it ended. Survives a
    /// restart of the system as far as the row does: a command that ended
    /// before the call is answered at once.
    pub async fn wait(&self, id: Id) -> Result<CommandOutcome, DispatchError> {
        let inner = &self.link.inner;
        // Told by the mirror that writes this command's end, and not by every other command's (C-turns:
        // a hundred waiting callers each read their row again whenever any command ended).
        let ended = self.link.end_signal(id);
        loop {
            let finished = inner.finished.notified();
            tokio::pin!(finished);
            finished.as_mut().enable();
            let own = ended.notified();
            tokio::pin!(own);
            own.as_mut().enable();
            let row = {
                let mut conn = inner.db.acquire().await?;
                queries::command(&mut conn, id).await?
            }
            .ok_or(DispatchError::UnknownCommand)?;
            if let Some(outcome) = CommandOutcome::of(&row) {
                return Ok(outcome);
            }
            tokio::select! {
                () = &mut own => {}
                () = &mut finished => {}
                () = tokio::time::sleep(WAIT_RECHECK) => {}
                () = inner.shutdown.cancelled() => return Err(DispatchError::Stopping),
            }
        }
    }

    /// Read only the canonical command's private custody facts for an existing retry.
    pub async fn retained_return_source(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::credentials::handover_authority::ReturnSource>, DispatchError>
    {
        let mut conn = self.link.inner.db.acquire().await?;
        let authority = queries::command_authority(&mut conn, id)
            .await?
            .map(serde_json::from_value::<orch_engine::commands::CommandAuthority>)
            .transpose()?;
        Ok(authority.and_then(|authority| authority.handover_source().cloned()))
    }

    /// Whether events `machine` sent on `stream` are at this link and not taken in yet
    /// ([`Link::hears_behind`], C-turns).
    #[must_use]
    pub fn hears_behind(&self, machine: Id, stream: &orch_link_contract::streams::Stream) -> bool {
        self.link.hears_behind(machine, stream)
    }

    /// Waits for a command of the catalogue and returns its result as the
    /// kind defines it.
    pub async fn wait_for<B: CommandBody>(&self, id: Id) -> Result<B::Result, RunError> {
        match self.wait(id).await? {
            CommandOutcome::Succeeded(body) => serde_json::from_value(body)
                .map_err(|error| CommandFailure::Unreadable(error.to_string()).into()),
            CommandOutcome::Failed(error) => Err(CommandFailure::Failed(error).into()),
            CommandOutcome::Expired => Err(CommandFailure::Expired.into()),
            CommandOutcome::Cancelled => Err(CommandFailure::Cancelled.into()),
        }
    }

    /// Issues a command of the catalogue and waits for its result. The answer
    /// of a read nobody reads twice (`ANSWERED_ONCE`) is then cleared from
    /// the command's row: the identifier was made here, and nobody else waits
    /// for it.
    pub async fn run<B: CommandBody>(
        &self,
        machine: Id,
        key: &SerialKey,
        body: &B,
    ) -> Result<B::Result, RunError> {
        let issued = self.issue(machine, key, body).await?;
        let answered = self.wait_for::<B>(issued.id).await;
        if ANSWERED_ONCE.contains(&B::KIND) {
            self.forget_answer(issued.id).await;
        }
        answered
    }

    /// Clears the answer of a command whose one reader has it. One that could not be cleared now stays
    /// until finished commands are removed, a day later.
    async fn forget_answer(&self, id: Id) {
        let cleared = match self.link.inner.db.acquire().await {
            Ok(mut conn) => queries::forget_result(&mut conn, id).await,
            Err(error) => Err(error),
        };
        if let Err(error) = cleared {
            tracing::debug!(%error, command = %id, "the answer of a read could not be cleared");
        }
    }

    /// The row of a command: where it stands, what it answered.
    pub async fn status(&self, id: Id) -> Result<Option<CommandRow>, DispatchError> {
        let mut conn = self.link.inner.db.acquire().await?;
        Ok(queries::command(&mut conn, id).await?)
    }

    /// Cancels a command that is not over. Returns whether it was: a
    /// machine that has the command is asked not to carry it out, and
    /// whatever it answers is not heard.
    /// A terminal controller failure replays stopping without replacing that first outcome.
    pub async fn cancel(&self, id: Id) -> Result<bool, DispatchError> {
        let inner = &self.link.inner;
        let row = {
            let mut conn = inner.db.acquire().await?;
            queries::command(&mut conn, id).await?
        }
        .ok_or(DispatchError::UnknownCommand)?;
        if row.is_over() {
            if matches!(row.state.as_str(), "failed" | "cancelled" | "expired")
                && let Some(entry) = self.link.loaded_entry(row.machine_id)
            {
                entry.controller.commands().cancel(id);
            }
            return Ok(false);
        }
        if let Some(entry) = self.link.loaded_entry(row.machine_id) {
            entry.controller.commands().cancel(id);
            self.link.sync_commands(&entry).await;
        }
        let mut conn = inner.db.acquire().await?;
        let ended =
            queries::finish_command(&mut conn, id, &Ending::Cancelled, inner.clock.now()).await?;
        if ended {
            inner.finished.notify_waiters();
        }
        Ok(true)
    }

    /// A controller failure or cancellation alone never proves that a native handler has stopped.
    pub async fn stopped(&self, id: Id) -> Result<bool, DispatchError> {
        let mut conn = self.link.inner.db.acquire().await?;
        Ok(sqlx::query_scalar::<_, bool>(
            "SELECT state='succeeded' OR
            (state IN ('failed','cancelled','expired') AND stop_confirmed_at IS NOT NULL) FROM commands WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?
        .unwrap_or(true))
    }
}

// ---------------------------------------------------------------- secrets

/// Why a secret's value could not be had.
#[derive(Debug, thiserror::Error)]
pub enum SecretFault {
    /// The row could not be read.
    #[error("{0}")]
    Db(#[from] DbError),
    /// The stored value does not open: another key, another row, or damage.
    #[error("the stored secret does not open")]
    Unreadable,
}

/// Where the values of the secrets that commands refer to come from.
#[async_trait]
pub trait SecretSource: Send + Sync {
    /// The value of the stored secret `id`, or none where there is no such
    /// secret.
    async fn value(&self, id: Id) -> Result<Option<Vec<u8>>, SecretFault>;
}

/// The secrets that the database keeps, sealed under the master key.
pub struct StoredSecrets {
    db: Db,
    master: MasterKey,
}

impl StoredSecrets {
    /// The secrets of `db`, opened with `master`.
    #[must_use]
    pub fn new(db: Db, master: MasterKey) -> Self {
        Self { db, master }
    }
}

impl std::fmt::Debug for StoredSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSecrets").finish_non_exhaustive()
    }
}

/// What a stored secret is bound to when it is sealed: its owner and its
/// name, so that a value does not open in another row. The same form the
/// settings write them in.
fn binding(owner_kind: &str, owner_id: Id, name: &str) -> Vec<u8> {
    format!("secrets/{owner_kind}/{owner_id}/{name}").into_bytes()
}

#[async_trait]
impl SecretSource for StoredSecrets {
    async fn value(&self, id: Id) -> Result<Option<Vec<u8>>, SecretFault> {
        let mut conn = self.db.acquire().await?;
        let Some(secret) = queries::stored_secret(&mut conn, id).await? else {
            if let Some(display) = orch_db::queries::displays::get(&mut conn, id).await?
                && display.state != "closed"
                && let Some(password) = display.password
            {
                return self
                    .master
                    .open(&password, format!("display/{id}/password").as_bytes())
                    .map(Some)
                    .map_err(|_| SecretFault::Unreadable);
            }
            return Ok(None);
        };
        self.master
            .open(
                &secret.sealed,
                &binding(&secret.owner_kind, secret.owner_id, &secret.name),
            )
            .map(Some)
            .map_err(|_| SecretFault::Unreadable)
    }
}

// ---------------------------------------------------------------- sending

impl Link {
    /// An adoption is only an output hint. The single machine output task reads
    /// its durable authority after external preparation and before publication.
    /// If cancellation races this read, its later queue fence cannot pass the
    /// current frame in this same ordered task. No database lease crosses I/O.
    pub async fn admit_wire_command(
        &self,
        entry: &MachineEntry,
        original: &Cmd,
        mut prepared: Cmd,
    ) -> Result<Option<(Cmd, Vec<(Option<orch_authz::Caller>, Option<Time>)>)>, CommandError> {
        let mut callers = Vec::new();
        let mut admission_tx = None;
        // Only these existing request-owned reads intentionally have no durable row.
        // The guard is registered before issue and removed on return/cancellation;
        // a kind string alone never grants an untracked command this exception.
        if (matches!(
            original.kind.as_str(),
            "session.background" | "session.context" | "session.native_actions"
        ) || original.kind == "session.open"
            && is_transport_rebind(&original.kind, &original.body))
            && entry.owns_observation(original.id)
        {
            return Ok(Some((prepared, callers)));
        }
        let private = {
            let mut conn = self.inner.db.acquire().await.map_err(|_| {
                CommandError::new(
                    ErrorCode::Internal,
                    "The durable command could not be checked before transmission.",
                )
            })?;
            queries::command_with_authority(&mut conn, original.id)
                .await
                .map_err(|_| {
                    CommandError::new(
                        ErrorCode::Internal,
                        "The durable command could not be checked before transmission.",
                    )
                })?
        }
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::NotFound,
                "The durable command is no longer available.",
            )
        })?;
        let has_authority = private.authority.is_some();
        let row = private.command;
        if row.machine_id != entry.id {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "The durable command belongs to another machine.",
            ));
        }
        if let Some(ended) = CommandOutcome::of(&row) {
            let terminal = match ended {
                CommandOutcome::Succeeded(body) => Progress::Succeeded(body),
                CommandOutcome::Failed(error) => Progress::Failed(error),
                CommandOutcome::Expired => Progress::Expired,
                CommandOutcome::Cancelled => Progress::Cancelled,
            };
            entry.reconcile_terminal(original.id, terminal);
            return Ok(None);
        }
        let canonical = cmd_of(&row);
        if canonical.kind != original.kind
            || canonical.key != original.key
            || canonical.body != original.body
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "The durable command identity changed before transmission.",
            ));
        }
        // Every command keeps the exact budget with which it was admitted.
        if canonical.deadline != original.deadline {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "The durable command deadline changed before transmission.",
            ));
        }
        prepared.deadline = canonical.deadline;
        if matches!(
            canonical.kind.as_str(),
            "provider.complete" | "provider.embed"
        ) {
            let kind = if canonical.kind == "provider.embed" {
                CommandKind::ProviderEmbed
            } else {
                CommandKind::ProviderComplete
            };
            let (provider, model) = provider_request(kind, &canonical.body).map_err(|_| {
                CommandError::new(ErrorCode::Conflict, "the provider request is invalid")
            })?;
            let mut conn = self.inner.db.acquire().await.map_err(|_| {
                CommandError::new(ErrorCode::Internal, "the provider cannot be checked")
            })?;
            if !orch_engine::resource_facts::activity::accepts_command(
                &mut conn,
                canonical.id,
                provider,
                &model,
            )
            .await
            .map_err(|_| CommandError::new(ErrorCode::Internal, "the provider cannot be checked"))?
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "Provider switched off",
                ));
            }
        }
        if row.state != "accepted" {
            let internal = || {
                CommandError::new(
                    ErrorCode::Internal,
                    "the creator source fence cannot be checked",
                )
            };
            let mut tx = self.inner.db.begin().await.map_err(|_| internal())?;
            orch_db::queries::transaction_guard::lock_action(&mut tx)
                .await
                .map_err(|_| internal())?;
            orch_engine::jobs::provision::lock_creator_command_source(&mut tx, row.id)
                .await
                .map_err(|_| internal())?;
            admission_tx = Some(tx);
        }
        // Native input publication follows the existing thread-before-command order of
        // Stop/admission. Only the DB fence is held; native transport remains after commit.
        if row.state != "accepted"
            && (matches!(canonical.kind.as_str(), "turn.start" | "turn.start.fenced")
                || (canonical.kind == "turn.steer" && has_authority))
        {
            let internal = || {
                CommandError::new(
                    ErrorCode::Internal,
                    "the native input fence cannot be checked",
                )
            };
            let mut tx = match admission_tx.take() {
                Some(tx) => tx,
                None => self.inner.db.begin().await.map_err(|_| internal())?,
            };
            orch_db::queries::transaction_guard::lock_action(&mut tx)
                .await
                .map_err(|_| internal())?;
            if let Some(thread) =
                orch_engine::inputs::native_delivery::thread_of(&row).map_err(|_| internal())?
            {
                orch_db::queries::threads::lock(&mut tx, thread)
                    .await
                    .map_err(|_| internal())?;
            }
            admission_tx = Some(tx);
        }
        // Preserve existing Login/Handover gating and apply the SAME private-channel checkpoint
        // to disjoint ordinary native origins. Provenance stays immutable; accepted restrictions
        // in an HTTP envelope may only narrow on the same locked canonical command.
        // An actual acknowledgement keeps result reconciliation separate from new authorization.
        if row.state != "accepted"
            && (has_authority
                || matches!(
                    canonical.kind.as_str(),
                    "provider.login.start"
                        | "provider.login.input"
                        | "provider.login.cancel"
                        | "provider.refresh"
                        | "agent.credential.return"
                        | "provider.credential.accept"
                ))
        {
            let operation = if matches!(
                canonical.kind.as_str(),
                "agent.credential.return" | "provider.credential.accept"
            ) {
                "credential return"
            } else if matches!(
                canonical.kind.as_str(),
                "provider.login.start"
                    | "provider.login.input"
                    | "provider.login.cancel"
                    | "provider.refresh"
            ) {
                "login"
            } else {
                "native operation"
            };
            let internal = || {
                CommandError::new(
                    ErrorCode::Internal,
                    format!("{operation} command authority cannot be checked"),
                )
            };
            let mut tx = match admission_tx.take() {
                Some(tx) => tx,
                None => self.inner.db.begin().await.map_err(|_| internal())?,
            };
            orch_db::queries::transaction_guard::lock_action(&mut tx)
                .await
                .map_err(|_| internal())?;
            if let Some(value) = &private.authority {
                let authority: orch_engine::commands::CommandAuthority =
                    serde_json::from_value(value.clone()).map_err(|_| internal())?;
                authority
                    .lock_native_source(&mut tx)
                    .await
                    .map_err(|_| internal())?;
            }
            let private = queries::lock_command_with_authority(&mut tx, row.id)
                .await
                .map_err(|_| internal())?
                .ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "the native command disappeared")
                })?;
            let current = private.command;
            let locked = cmd_of(&current);
            if current.machine_id != row.machine_id
                || locked.kind != canonical.kind
                || locked.key != canonical.key
                || locked.body != canonical.body
                || locked.deadline != canonical.deadline
            {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the canonical native command changed during admission",
                ));
            }
            if let Some(ended) = CommandOutcome::of(&current) {
                let terminal = match ended {
                    CommandOutcome::Succeeded(body) => Progress::Succeeded(body),
                    CommandOutcome::Failed(error) => Progress::Failed(error),
                    CommandOutcome::Expired => Progress::Expired,
                    CommandOutcome::Cancelled => Progress::Cancelled,
                };
                entry.reconcile_terminal(original.id, terminal);
                return Ok(None);
            }
            if current.state != "accepted"
                && let Some(stored) = private.authority
            {
                let authority: orch_engine::commands::CommandAuthority =
                    serde_json::from_value(stored).map_err(|_| {
                        CommandError::new(
                            ErrorCode::Conflict,
                            format!("{operation} command authority is invalid"),
                        )
                    })?;
                let Some(checked) = authority
                    .current_callers(
                        &mut tx,
                        &current,
                        self.inner.clock.as_ref(),
                        &crate::thread_access::AuthzAccess::new(self.inner.clock.clone()),
                    )
                    .await
                    .map_err(|_| internal())?
                else {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        format!("the accepted {operation} command is no longer authorized"),
                    ));
                };
                callers.extend(checked);
            }
            // Keep original/current key row boundaries through later currentness validators.
            // This transaction still contains DB-only admission; publication remains outside it.
            admission_tx = Some(tx);
        }
        if row.state != "accepted" {
            let internal = || {
                CommandError::new(
                    ErrorCode::Internal,
                    "creator operation authority cannot be checked",
                )
            };
            let mut tx = match admission_tx.take() {
                Some(tx) => tx,
                None => self.inner.db.begin().await.map_err(|_| internal())?,
            };
            orch_db::queries::transaction_guard::lock_action(&mut tx)
                .await
                .map_err(|_| internal())?;
            let current = queries::lock_command(&mut tx, row.id)
                .await
                .map_err(|_| internal())?
                .ok_or_else(|| {
                    CommandError::new(ErrorCode::NotFound, "the creator command disappeared")
                })?;
            if let Some(ended) = CommandOutcome::of(&current) {
                let terminal = match ended {
                    CommandOutcome::Succeeded(body) => Progress::Succeeded(body),
                    CommandOutcome::Failed(error) => Progress::Failed(error),
                    CommandOutcome::Expired => Progress::Expired,
                    CommandOutcome::Cancelled => Progress::Cancelled,
                };
                entry.reconcile_terminal(original.id, terminal);
                return Ok(None);
            }
            if current.machine_id != row.machine_id || cmd_of(&current) != canonical {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the canonical native command changed before current publication",
                ));
            }
            if current.state == "accepted" {
                callers.clear();
            }
            if current.state != "accepted" {
                let input_callers =
                    if matches!(canonical.kind.as_str(), "turn.start" | "turn.start.fenced") {
                        orch_engine::inputs::native_delivery::current_callers(
                            &mut tx,
                            &current,
                            None,
                            self.inner.clock.as_ref(),
                            &crate::thread_access::AuthzAccess::new(self.inner.clock.clone()),
                        )
                        .await
                        .map_err(|_| internal())?
                    } else {
                        Some(Vec::new())
                    };
                let Some(input_callers) = input_callers else {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the accepted input operation is no longer authorized",
                    ));
                };
                callers.extend(input_callers);
                let file_callers = if matches!(
                    canonical.kind.as_str(),
                    "fs.snapshot" | "fs.record.read" | "fs.materialize"
                ) {
                    orch_engine::initial_files::command_current_callers(
                        &mut tx,
                        current.id,
                        current.machine_id,
                        canonical.key.as_deref(),
                        &canonical.kind,
                        &canonical.body,
                        self.inner.clock.as_ref(),
                    )
                    .await
                } else if matches!(
                    canonical.kind.as_str(),
                    "fs.stat"
                        | "fs.mkdir"
                        | "fs.record"
                        | "git.prepare"
                        | "git.worktree"
                        | "git.commit"
                        | "git.push"
                ) {
                    orch_engine::jobs::source::creator_command_current_callers(
                        &mut tx,
                        current.id,
                        current.machine_id,
                        canonical.key.as_deref(),
                        &canonical.kind,
                        &canonical.body,
                        self.inner.clock.as_ref(),
                    )
                    .await
                } else {
                    Ok(Some(Vec::new()))
                }
                .map_err(|_| internal())?;
                // A legacy/lost private tag still has the capability job's actual immutable
                // journal association. Known malformed/missing origins never become ordinary.
                let capability_callers =
                    if canonical.kind == "node.capabilities.apply" && !has_authority {
                        orch_engine::jobs::capability_change::authority::command_current_callers(
                            &mut tx,
                            current.id,
                            current.machine_id,
                            canonical.key.as_deref(),
                            &canonical.kind,
                            &canonical.body,
                            self.inner.clock.as_ref(),
                        )
                        .await
                        .map_err(|_| internal())?
                    } else {
                        Some(Vec::new())
                    };
                let Some(capability_callers) = capability_callers else {
                    return Err(CommandError::new(
                        ErrorCode::Conflict,
                        "the accepted live capability operation is no longer authorized",
                    ));
                };
                callers.extend(capability_callers);
                let provision_callers =
                    orch_engine::jobs::provision::creator_command_current_callers(
                        &mut tx,
                        current.id,
                        current.machine_id,
                        canonical.key.as_deref(),
                        &canonical.kind,
                        &canonical.body,
                        self.inner.clock.as_ref(),
                        &crate::thread_access::AuthzAccess::new(self.inner.clock.clone()),
                    )
                    .await
                    .map_err(|_| internal())?;
                let provider_callers =
                    orch_engine::jobs::provider_provision::creator_command_current_callers(
                        &mut tx,
                        current.id,
                        current.machine_id,
                        canonical.key.as_deref(),
                        &canonical.kind,
                        &canonical.body,
                        self.inner.clock.as_ref(),
                    )
                    .await
                    .map_err(|_| internal())?;
                let batch_callers =
                    orch_engine::jobs::recreate::batch_authority::command_current_callers(
                        &mut tx,
                        current.id,
                        current.machine_id,
                        canonical.key.as_deref(),
                        &canonical.kind,
                        &canonical.body,
                        self.inner.clock.as_ref(),
                    )
                    .await
                    .map_err(|_| internal())?;
                match (
                    file_callers,
                    provision_callers,
                    provider_callers,
                    batch_callers,
                ) {
                    (Some(file), Some(provision), Some(provider), Some(batch)) => {
                        callers.extend(file);
                        callers.extend(provision);
                        callers.extend(provider);
                        callers.extend(batch);
                    }
                    _ => {
                        return Err(CommandError::new(
                            ErrorCode::Conflict,
                            "the accepted creator operation is no longer authorized",
                        ));
                    }
                }
            }
            tx.commit().await.map_err(|_| internal())?;
        }
        #[cfg(any(test, feature = "test-support"))]
        crate::backplane::command_admission_tests::wire_checked(original.id, entry.id).await;
        Ok(Some((prepared, callers)))
    }

    /// The command as it goes on the wire: every reference in its body
    /// replaced, a secret by its value sealed to the machine's key, a ticket
    /// by one signed now. A command that cannot be made whole is not sent;
    /// the error is what the command ends with.
    pub async fn prepare(
        &self,
        entry: &MachineEntry,
        session_fingerprint: Option<&str>,
        mut cmd: Cmd,
    ) -> Result<Cmd, CommandError> {
        if !has_references(&cmd.body) {
            return Ok(cmd);
        }
        let mut wanted: Vec<Id> = Vec::new();
        let mut ticket_requests: Vec<serde_json::Map<String, Json>> = Vec::new();
        let mut scan = cmd.body.clone();
        // Collecting the secrets' ids cannot fail, which the error type says.
        let Ok(()) = resolve_references(&mut scan, &mut |reference| {
            match reference {
                Reference::Secret(id) => wanted.push(id),
                Reference::Ticket(more) => ticket_requests.push(more.clone()),
            }
            Ok::<_, std::convert::Infallible>(String::new())
        });
        let inner = &self.inner;
        let mut sealed: HashMap<Id, String> = HashMap::new();
        if !wanted.is_empty() {
            let key = self.recipient_key(entry.id, session_fingerprint).await?;
            let source = std::sync::Arc::clone(
                &inner
                    .secrets
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            for id in wanted {
                if sealed.contains_key(&id) {
                    continue;
                }
                let value = source
                    .value(id)
                    .await
                    .map_err(|error| {
                        CommandError::new(
                            ErrorCode::Internal,
                            format!("a secret cannot be read: {error}"),
                        )
                    })?
                    .ok_or_else(|| {
                        CommandError::new(
                            ErrorCode::NotFound,
                            "a secret the command refers to is gone",
                        )
                    })?;
                let text = seal(&key, &info_for_command(cmd.id), &value).map_err(|error| {
                    CommandError::new(
                        ErrorCode::Internal,
                        format!("a secret cannot be sealed: {error}"),
                    )
                })?;
                sealed.insert(id, text);
            }
        }
        let machine = entry.id;
        // Resolve transport policy before the synchronous substitution, with exact reference identities.
        // Every occurrence still receives its own newly signed ticket and frozen plan.
        let mut routed_tickets = std::collections::VecDeque::new();
        for more in ticket_requests {
            let wanted: TicketWanted =
                serde_json::from_value(Json::Object(more.clone())).map_err(|error| {
                    CommandError::new(
                        ErrorCode::Internal,
                        format!("a ticket cannot be asked for: {error}"),
                    )
                })?;
            let ticket = inner
                .tickets
                .for_request(&wanted, machine)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
            routed_tickets.push_back((more, self.routed_ticket_value(ticket).await?));
        }
        resolve_reference_values(&mut cmd.body, &mut |reference| match reference {
            Reference::Secret(id) => {
                sealed.get(&id).cloned().map(Json::String).ok_or_else(|| {
                    CommandError::new(ErrorCode::Internal, "a secret was not sealed")
                })
            }
            Reference::Ticket(more) => match routed_tickets.pop_front() {
                Some((expected, value)) if expected == more.clone() => Ok(value),
                _ => Err(CommandError::new(
                    ErrorCode::Internal,
                    "the ticket reference changed while being prepared",
                )),
            },
        })?;
        Ok(cmd)
    }

    /// The key that secrets are sealed to for the session a machine has: the
    /// public key of the certificate it opened the session with.
    async fn recipient_key(
        &self,
        machine: Id,
        session_fingerprint: Option<&str>,
    ) -> Result<RecipientKey, CommandError> {
        let unknown = |why: &str| CommandError::new(ErrorCode::Internal, why.to_owned());
        let mut conn = self
            .inner
            .db
            .acquire()
            .await
            .map_err(|_| unknown("the machine's key could not be looked up"))?;
        let row = orch_db::queries::resources::machine(&mut conn, machine)
            .await
            .map_err(|_| unknown("the machine's key could not be looked up"))?
            .ok_or_else(|| unknown("the machine is not known"))?;
        let link = row.link();
        let text = match (&link.next, session_fingerprint) {
            (Some(next), Some(fingerprint)) if next.fingerprint == fingerprint => {
                next.public_key.clone()
            }
            _ => link.public_key,
        };
        RecipientKey::parse(&text).map_err(|_| unknown("the machine's key is not known"))
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use crate::backplane::LinkParts;

    use orch_config::ServerConfig;

    use orch_db::audit::Actor;

    use orch_db::audit::Origin;

    use orch_db::queries::hierarchy;

    use orch_db::queries::resources;

    use orch_db::queries::resources::NewMachine;

    use orch_db::queries::resources::testing;

    use orch_link_contract::tunnels::TicketSource;

    use orch_pki::ca::CertificateAuthority;

    use orch_pki::ca::Holder;

    use orch_pki::sealing::open;

    use orch_settings::secrets::Owner;

    use orch_settings::secrets::Put;

    use orch_testkit::database::TestDb;

    use orch_types::Clock;

    use orch_types::ManualClock;

    use serde_json::json;

    use std::sync::Arc;
    struct Setup {
        _test: TestDb,
        db: Db,
        clock: ManualClock,
        link: Link,
        machine: Id,
        key: orch_pki::certificates::LeafKey,
        master: [u8; 32],
    }

    /// A link over a database, with a machine whose certificate the system
    /// issued, so that there is a key to seal to.
    async fn setup() -> Setup {
        let test = TestDb::fresh().await;
        let db = Db::from_pool(test.pool.clone());
        let clock = ManualClock::new(orch_testkit::clock::start());
        let authority = Arc::new(CertificateAuthority::create("Test", 3_650, &clock).unwrap());
        let mut master = [0u8; 32];
        rand::fill(&mut master[..]);
        let link = Link::new(LinkParts {
            db: db.clone(),
            clock: Arc::new(clock.clone()),
            config: Arc::new(ServerConfig::default()),
            release_version: "0.1.0".to_owned(),
            authority: Arc::clone(&authority),
            master_key: MasterKey::new(master),
        });
        let machine = new_id();
        let (key, certificate) = authority
            .issue_with_new_key(&Holder::Machine(machine), 365, &clock)
            .unwrap();
        {
            let mut conn = db.acquire().await.unwrap();
            resources::insert_machine(
                &mut conn,
                &NewMachine {
                    id: machine,
                    name: "m",
                    owner_id: None,
                    placement: "connected",
                    roles: &[],
                    platform_os: "linux",
                    platform_arch: "x86_64",
                    connector_version: "0.1.0",
                    protocol_version: 1,
                    cert_serial: "01",
                    cert_fingerprint: &hex::decode(certificate.fingerprint()).unwrap(),
                    cert_not_after: certificate.not_after().get(),
                    public_key: &orch_pki::sealing::RecipientKey::from_certificate(&certificate)
                        .to_text(),
                },
                clock.now(),
            )
            .await
            .unwrap();
        }
        Setup {
            _test: test,
            db,
            clock,
            link,
            machine,
            key,
            master,
        }
    }

    fn command(body: Json) -> Cmd {
        Cmd {
            id: new_id(),
            kind: "provider.start".to_owned(),
            key: None,
            deadline: Time::new(orch_testkit::clock::start()),
            body,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_stored_command_carries_its_key_and_gives_it_back() {
        let body = stored_body(json!({ "thread_id": "t" }), Some("machine"));
        assert_eq!(body["$key"], "machine");
        let mut row = CommandRow {
            id: new_id(),
            machine_id: new_id(),
            kind: "machine.probe".into(),
            body,
            serial_key: None,
            state: "queued".into(),
            result: None,
            error: None,
            attempts: 0,
            deadline_at: orch_testkit::clock::start(),
            created_at: orch_testkit::clock::start(),
            updated_at: orch_testkit::clock::start(),
        };
        let cmd = cmd_of(&row);
        assert_eq!(cmd.key.as_deref(), Some("machine"));
        assert_eq!(
            cmd.body,
            json!({ "thread_id": "t" }),
            "the key is not sent in the body"
        );
        row.serial_key = Some("protected".to_owned());
        row.body[KEY_MEMBER] = json!("stale");
        let cmd = cmd_of(&row);
        assert_eq!(cmd.key.as_deref(), Some("protected"));
        assert_eq!(cmd.body, json!({ "thread_id":"t" }));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_stored_command_carries_its_key_and_gives_it_back() {
        self::a_stored_command_carries_its_key_and_gives_it_back()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn a_command_without_a_key_has_none() {
        let body = stored_body(json!({}), None);
        assert_eq!(body, json!({}));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_command_without_a_key_has_none() {
        self::a_command_without_a_key_has_none()
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn a_ticket_request_in_a_command_is_signed_when_the_command_is_sent() {
        let setup = setup().await;
        let entry = setup.link.entry(setup.machine).await.unwrap();
        let environment = new_id();
        let stored = command(json!({
            "exec": { "ticket": { "$ticket": { "kind": "exec", "service": "exec/codex", "dst": environment, "environment_id": environment, "environment_generation": 3 } } }
        }));
        assert!(has_references(&stored.body));
        let sent = setup.link.prepare(&entry, None, stored).await.unwrap();
        assert!(
            !has_references(&sent.body),
            "nothing of the stored form is left on the wire"
        );
        let ticket = sent.body["exec"]["ticket"].as_str().unwrap();
        let claims = setup
            .link
            .tickets()
            .verifier()
            .verify(
                ticket,
                TicketSource::Machine(setup.machine),
                Time::new(setup.clock.now()),
            )
            .unwrap();
        assert_eq!(
            (claims.dst, claims.service.as_str()),
            (environment, "exec/codex")
        );
        assert_eq!(claims.tool_environment_id, Some(environment));
        assert_eq!(claims.tool_environment_generation, Some(3));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_ticket_request_in_a_command_is_signed_when_the_command_is_sent()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(
            self::a_ticket_request_in_a_command_is_signed_when_the_command_is_sent(),
        )
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it()
     {
        let setup = setup().await;
        let entry = setup.link.entry(setup.machine).await.unwrap();
        // Written by the settings, as the people who set a provider's key write it.
        let org = new_id();
        let secret: Id = {
            let mut conn = setup.db.acquire().await.unwrap();
            testing::insert_user(&mut conn, "ada").await.unwrap();
            let user = sqlx::query_scalar("SELECT id FROM users WHERE username = 'ada'")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            hierarchy::testing::insert_node(&mut conn, org, "org", None, "acme", user)
                .await
                .unwrap();
            orch_settings::secrets::put(
                &mut conn,
                &MasterKey::new(setup.master),
                &Put {
                    actor: Actor::User(user),
                    owner: Owner::Node(org),
                    name: "PROVIDER_API_KEY",
                    value: "sk-live-123",
                    origin: &Origin::default(),
                },
                setup.clock.now(),
            )
            .await
            .unwrap();
            sqlx::query_scalar(
                "SELECT id FROM secrets WHERE owner_id = $1 AND name = 'PROVIDER_API_KEY'",
            )
            .bind(org)
            .fetch_one(&mut *conn)
            .await
            .unwrap()
        };
        let stored = command(json!({ "credential": { "$secret": secret } }));
        let id = stored.id;
        let sent = setup.link.prepare(&entry, None, stored).await.unwrap();
        let sealed = sent.body["credential"]["$sealed"].as_str().unwrap();
        assert!(!sealed.contains("sk-live"));
        assert_eq!(
            open(&setup.key, &info_for_command(id), sealed).unwrap(),
            b"sk-live-123"
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(self::a_secret_that_the_settings_stored_is_sealed_to_the_machine_for_the_command_that_names_it())
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn a_command_that_cannot_be_made_whole_is_not_sent_and_says_why() {
        let setup = setup().await;
        let entry = setup.link.entry(setup.machine).await.unwrap();
        // A secret that is gone.
        let gone = command(json!({ "credential": { "$secret": new_id() } }));
        let error = setup.link.prepare(&entry, None, gone).await.unwrap_err();
        assert_eq!(error.code, "not_found");
        // A ticket that asks for something that is no ticket.
        let nonsense = command(
            json!({ "ticket": { "$ticket": { "kind": "llm", "service": "exec/codex", "dst": new_id() } } }),
        );
        let error = setup
            .link
            .prepare(&entry, None, nonsense)
            .await
            .unwrap_err();
        assert_eq!(error.code, "internal");
        assert!(error.detail.contains("kind"), "{}", error.detail);
        // A body with nothing to replace is sent as it is.
        let plain = command(json!({ "thread_id": "t" }));
        assert_eq!(
            setup
                .link
                .prepare(&entry, None, plain.clone())
                .await
                .unwrap(),
            plain
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_a_command_that_cannot_be_made_whole_is_not_sent_and_says_why()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(self::a_command_that_cannot_be_made_whole_is_not_sent_and_says_why())
    }

    /// Answers the command of `kind` once it has been asked, as its machine would, and says which it was.
    async fn answer(setup: &Setup, kind: &str, result: Json) -> Id {
        loop {
            let mut conn = setup.db.acquire().await.unwrap();
            let asked: Option<Id> = sqlx::query_scalar(
                "SELECT id FROM commands WHERE kind = $1 AND state IN ('queued', 'sent', 'accepted')",
            )
            .bind(kind)
            .fetch_optional(&mut *conn)
            .await
            .unwrap();
            if let Some(id) = asked {
                let ending = Ending::Succeeded(&result);
                queries::finish_command(&mut conn, id, &ending, setup.clock.now())
                    .await
                    .unwrap();
                setup.link.inner.finished.notify_waiters();
                return id;
            }
            drop(conn);
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    async fn the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired() {
        let setup = setup().await;
        let dispatcher = setup.link.dispatcher();
        let environment = new_id();
        let listing = tokio::spawn({
            let (dispatcher, machine) = (dispatcher.clone(), setup.machine);
            async move {
                let body = orch_link_contract::commands::FsList {
                    environment_id: environment,
                    mount: None,
                    path: String::new(),
                    watch_id: None,
                    watch_instance: None,
                    page: None,
                };
                dispatcher.run(machine, &SerialKey::None, &body).await
            }
        });
        let id = answer(
            &setup,
            "fs.list",
            json!({ "entries": [], "truncated": true }),
        )
        .await;
        let listed = listing.await.unwrap().unwrap();
        assert!(listed.truncated, "the one who asked has the answer");
        let row = {
            let mut conn = setup.db.acquire().await.unwrap();
            queries::command(&mut conn, id).await.unwrap().unwrap()
        };
        assert_eq!(
            (row.state.as_str(), row.result),
            ("succeeded", None),
            "the row says that it ran, without the listing"
        );
        assert_eq!(dispatcher.wait(id).await.unwrap(), CommandOutcome::Expired);
        assert!(matches!(
            dispatcher
                .wait_for::<orch_link_contract::commands::FsList>(id)
                .await,
            Err(RunError::Command(CommandFailure::Expired))
        ));

        // What is not one of those reads keeps its answer.
        let watching = tokio::spawn({
            let (dispatcher, machine) = (dispatcher.clone(), setup.machine);
            async move {
                let body = orch_link_contract::commands::WatchSet {
                    environment_id: environment,
                    stats: true,
                    procs: false,
                    ports: false,
                    terminals: false,
                    interval_ms: 1_000,
                };
                dispatcher.run(machine, &SerialKey::None, &body).await
            }
        });
        let kept = answer(&setup, "watch.set", json!({})).await;
        watching.await.unwrap().unwrap();
        assert_eq!(
            dispatcher.wait(kept).await.unwrap(),
            CommandOutcome::Succeeded(json!({}))
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired()
    -> ::futures::future::LocalBoxFuture<'static, ()> {
        ::std::boxed::Box::pin(self::the_answer_of_a_read_is_cleared_once_had_and_a_late_reader_is_told_that_it_expired())
    }
}
