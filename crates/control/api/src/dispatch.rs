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

use crate::backplane::{Link, MachineEntry};
use async_trait::async_trait;
use chrono::Duration as ChronoDuration;
use orch_db::queries::link::{self as queries, CommandRow, Ending, NewCommand};
use orch_db::secrets::MasterKey;
use orch_db::{Db, DbError};
use orch_link::commands::{CommandFailure, Progress};
use orch_link_contract::commands::{
    Cmd, CommandBody, CommandError, CommandKind, ErrorCode, Reference, SerialKey, TicketWanted,
    has_references, resolve_references,
};
use orch_pki::sealing::{RecipientKey, info_for_command, seal};
use orch_types::{Id, Json, Time, new_id};
use std::collections::HashMap;
use std::time::Duration;

/// The older stored member read only when no protected serial key was supplied.
pub const KEY_MEMBER: &str = "$key";

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

#[cfg(test)]
fn stored_body(mut body: Json, key: Option<&str>) -> Json {
    if let (Some(key), Some(members)) = (key, body.as_object_mut()) {
        members.insert(KEY_MEMBER.to_owned(), Json::String(key.to_owned()));
    }
    body
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

    pub(crate) fn new(link: Link) -> Self {
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
        if command.kind == "session.background" {
            return Err(DispatchError::UnknownKind(
                "live native reads must use the transient observation path".to_owned(),
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
        let body = command.body;
        let mut tx = inner.db.begin().await?;
        let provider_allowed = if matches!(
            kind,
            CommandKind::ProviderComplete | CommandKind::ProviderEmbed
        ) {
            orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
            let (provider, model) = provider_request(kind, &body)?;
            orch_engine::resource_facts::activity::accepts_command(&mut tx, id, provider, &model)
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
        let inserted = queries::insert_command(
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
        )
        .await?;
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
        tx.commit().await.map_err(DbError::from)?;
        // Lifecycle transactions may have committed this command before the dispatcher sees it.
        #[cfg(test)]
        crate::backplane::command_admission_tests::adoption_read(&self.link, id).await;
        if row.state == "queued" {
            entry.adopt(cmd_of(&row), Progress::Queued);
        }
        Ok(Issued {
            id,
            deadline: Time::new(row.deadline_at),
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
    pub(crate) async fn admit_wire_command(
        &self,
        entry: &MachineEntry,
        original: &Cmd,
        mut prepared: Cmd,
    ) -> Result<Option<Cmd>, CommandError> {
        // Only these existing request-owned reads intentionally have no durable row.
        // The guard is registered before issue and removed on return/cancellation;
        // a kind string alone never grants an untracked command this exception.
        if matches!(
            original.kind.as_str(),
            "session.background" | "session.context" | "session.native_actions"
        ) && entry.owns_observation(original.id)
        {
            return Ok(Some(prepared));
        }
        let row = {
            let mut conn = self.inner.db.acquire().await.map_err(|_| {
                CommandError::new(
                    ErrorCode::Internal,
                    "The durable command could not be checked before transmission.",
                )
            })?;
            queries::command(&mut conn, original.id)
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
        #[cfg(test)]
        crate::backplane::command_admission_tests::wire_checked(original.id).await;
        Ok(Some(prepared))
    }

    /// The command as it goes on the wire: every reference in its body
    /// replaced, a secret by its value sealed to the machine's key, a ticket
    /// by one signed now. A command that cannot be made whole is not sent;
    /// the error is what the command ends with.
    pub(crate) async fn prepare(
        &self,
        entry: &MachineEntry,
        session_fingerprint: Option<&str>,
        mut cmd: Cmd,
    ) -> Result<Cmd, CommandError> {
        if !has_references(&cmd.body) {
            return Ok(cmd);
        }
        let mut wanted: Vec<Id> = Vec::new();
        let mut scan = cmd.body.clone();
        // Collecting the secrets' ids cannot fail, which the error type says.
        let Ok(()) = resolve_references(&mut scan, &mut |reference| {
            if let Reference::Secret(id) = reference {
                wanted.push(id);
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
        resolve_references(&mut cmd.body, &mut |reference| match reference {
            Reference::Secret(id) => sealed
                .get(&id)
                .cloned()
                .ok_or_else(|| CommandError::new(ErrorCode::Internal, "a secret was not sealed")),
            Reference::Ticket(more) => {
                let wanted: TicketWanted = serde_json::from_value(Json::Object(more.clone()))
                    .map_err(|error| {
                        CommandError::new(
                            ErrorCode::Internal,
                            format!("a ticket cannot be asked for: {error}"),
                        )
                    })?;
                inner
                    .tickets
                    .for_request(&wanted, machine)
                    .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backplane::LinkParts;
    use orch_config::ServerConfig;
    use orch_db::audit::{Actor, Origin};
    use orch_db::queries::hierarchy;
    use orch_db::queries::resources::{self, NewMachine, testing};
    use orch_link_contract::tunnels::TicketSource;
    use orch_pki::ca::{CertificateAuthority, Holder};
    use orch_pki::sealing::open;
    use orch_settings::secrets::{Owner, Put};
    use orch_testkit::database::TestDb;
    use orch_types::{Clock, ManualClock};
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

    #[test]
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

    #[test]
    fn a_command_without_a_key_has_none() {
        let body = stored_body(json!({}), None);
        assert_eq!(body, json!({}));
    }

    #[tokio::test]
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

    #[tokio::test]
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

    #[tokio::test]
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

    #[tokio::test]
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
}
