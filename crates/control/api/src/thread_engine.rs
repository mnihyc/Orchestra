//! The thread engine inside the API: its commands go through the link's dispatcher, a machine's events are applied by it, and it is built once at start.
//!
//! The engine asks three things of the program it lives in, each through a
//! small trait of its own. Here the first is answered: [`DispatchCommands`]
//! sends a command as a row that is resent until the machine has answered
//! ([`crate::dispatch`]), and tells how it stands. [`EngineApplier`] is the
//! other direction: registered with the link, it hands the events of
//! the session and harness streams to the engine inside the transaction that
//! moves their cursor. What the engine asks about resources is answered by
//! the access engine ([`crate::thread_access`]). The third port is not
//! answered yet and stands in as the engine's own default: growing text is
//! not forwarded to anyone.

use crate::backplane::{ApplyError, EventApplier, Link};
use crate::dispatch::{CommandOutcome, DispatchError, Dispatcher, StoredCommand};
use crate::hosting::{ApiHostServices, ReleaseBuilds};
use async_trait::async_trait;
use orch_blob_contract::store::BlobStore;
use orch_config::ServerConfig;
use orch_db::{Conn, Db};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_engine::config::EngineConfig;
use orch_engine::live::{Discard, LiveSink};
use orch_engine::{Engine, EngineError, EngineParts};
use orch_link::streams::Delivery;
use orch_link_contract::streams::{ResetReason, Stream};
use orch_types::{Clock, Id};
use std::sync::Arc;

/// Sends the engine's commands through the dispatcher of the link.
#[derive(Debug, Clone)]
pub struct DispatchCommands {
    dispatcher: Dispatcher,
}

impl DispatchCommands {
    /// The engine's commands, sent by `dispatcher`.
    #[must_use]
    pub const fn new(dispatcher: Dispatcher) -> Self {
        Self { dispatcher }
    }
}

/// How a command ended, in the engine's words.
fn outcome_of(outcome: CommandOutcome) -> Outcome {
    match outcome {
        CommandOutcome::Succeeded(result) => Outcome::Succeeded(result),
        CommandOutcome::Failed(error) => Outcome::Failed(error),
        CommandOutcome::Expired => Outcome::Expired,
        CommandOutcome::Cancelled => Outcome::Cancelled,
    }
}

/// Why a command could not be handled, in the engine's words.
fn commands_error(error: DispatchError) -> CommandsError {
    match error {
        DispatchError::UnknownMachine => CommandsError::UnknownMachine,
        DispatchError::Revoked => CommandsError::Revoked,
        DispatchError::Stopping => CommandsError::Stopping,
        DispatchError::Body(error) => CommandsError::Body(error.to_string()),
        other => CommandsError::Other(other.to_string()),
    }
}

/// Where a command stands, from the word its row keeps.
fn stage_of(state: &str) -> Stage {
    match state {
        "queued" => Stage::Queued,
        "sent" => Stage::Sent,
        "accepted" => Stage::Accepted,
        _ => Stage::Over,
    }
}

#[async_trait]
impl Commands for DispatchCommands {
    async fn retained_body(&self, id: Id) -> Result<Option<orch_types::Json>, CommandsError> {
        Ok(self
            .dispatcher
            .status(id)
            .await
            .map_err(commands_error)?
            .map(|row| row.body))
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        Ok(self
            .dispatcher
            .status(id)
            .await
            .map_err(commands_error)?
            .map(|row| orch_engine::commands::RetainedCommand {
                machine: row.machine_id,
                body: row.body,
            }))
    }
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        if command.kind == orch_link_contract::commands::CommandKind::SessionNativeActions {
            let body = serde_json::from_value(command.body)
                .map_err(|error| CommandsError::Body(error.to_string()))?;
            return self
                .dispatcher
                .observe_native_actions(command.machine, &body)
                .await
                .map(Outcome::Succeeded)
                .map_err(commands_error);
        }
        if command.kind == orch_link_contract::commands::CommandKind::SessionContext {
            // A count of what fills a session's context is read live as well, and kept by the engine itself.
            let body = serde_json::from_value(command.body)
                .map_err(|error| CommandsError::Body(error.to_string()))?;
            return self
                .dispatcher
                .observe_context(command.machine, &body)
                .await
                .map(Outcome::Succeeded)
                .map_err(commands_error);
        }
        if command.kind != orch_link_contract::commands::CommandKind::SessionBackground {
            return Err(CommandsError::Body(
                "only native session facts use this transient path".to_owned(),
            ));
        }
        let body = serde_json::from_value(command.body)
            .map_err(|error| CommandsError::Body(error.to_string()))?;
        self.dispatcher
            .observe_background(command.machine, &body)
            .await
            .map(Outcome::Succeeded)
            .map_err(commands_error)
    }

    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        self.dispatcher
            .issue_stored(StoredCommand {
                id: Some(command.id),
                machine: command.machine,
                kind: command.kind.as_str().to_owned(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body,
            })
            .await
            .map(|_| ())
            .map_err(commands_error)
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        let Some(row) = self.dispatcher.status(id).await.map_err(commands_error)? else {
            return Ok(None);
        };
        let stage = stage_of(&row.state);
        // A command that is over answers at once.
        let outcome = if stage == Stage::Over {
            Some(outcome_of(
                self.dispatcher.wait(id).await.map_err(commands_error)?,
            ))
        } else {
            None
        };
        Ok(Some(Info {
            stage,
            issued_at: row.created_at,
            outcome,
        }))
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.dispatcher
            .wait(id)
            .await
            .map(outcome_of)
            .map_err(commands_error)
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        match self.dispatcher.cancel(id).await {
            Ok(was_open) => Ok(was_open),
            Err(DispatchError::UnknownCommand) => Ok(false),
            Err(error) => Err(commands_error(error)),
        }
    }

    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        self.dispatcher.stopped(id).await.map_err(commands_error)
    }

    fn hears_behind(&self, machine: Id, stream: &Stream) -> bool {
        self.dispatcher.hears_behind(machine, stream)
    }
}

/// Applies the events of the session and harness streams of machines with
/// the thread engine.
#[derive(Debug, Clone)]
pub struct EngineApplier {
    engine: Engine,
}

impl EngineApplier {
    /// The events of machines, applied by `engine`.
    #[must_use]
    pub const fn new(engine: Engine) -> Self {
        Self { engine }
    }
}

/// What the link is told when the engine could not apply events: a failed
/// database is the database's, and anything else sends the events again.
fn apply_error(error: EngineError) -> ApplyError {
    match error {
        EngineError::Db(error) => ApplyError::Db(error),
        other => ApplyError::Other(other.to_string()),
    }
}

#[async_trait]
impl EventApplier for EngineApplier {
    fn component_update_finished(&self, machine: Id) {
        let engine = self.engine.clone();
        tokio::spawn(async move {
            if let Err(error) = engine.component_update_finished(machine).await {
                tracing::warn!(%error,%machine,"work waiting on a physical update could not be resumed");
            }
        });
    }
    async fn discover(&self, machine: Id, retry: bool) -> Result<(), ApplyError> {
        self.engine
            .discover_descriptors_after_facts(machine, retry)
            .await
            .map_err(apply_error)
    }

    fn committed(&self, _machine: Id, delivery: &Delivery) {
        if let Stream::Env { environment_id } = delivery.stream {
            let browser_changed = delivery.events.iter().any(|e| e.kind == "browser.state");
            let display_changed = delivery.events.iter().any(|e| e.kind == "display.state");
            let attached = delivery
                .events
                .iter()
                .any(|e| e.kind == "env.status" && e.body["status"] == "running");
            if browser_changed || display_changed || attached {
                let engine = self.engine.clone();
                tokio::spawn(async move {
                    if attached {
                        let _ = engine.browsers().reconcile(environment_id).await;
                    } else {
                        engine.browsers().changed(environment_id).await;
                    }
                    if display_changed || attached {
                        engine.displays().changed(environment_id).await;
                    }
                });
            }
        }
    }

    async fn apply(
        &self,
        tx: &mut Conn,
        machine: Id,
        delivery: &Delivery,
    ) -> Result<(), ApplyError> {
        self.engine
            .apply_events(tx, machine, delivery)
            .await
            .map_err(apply_error)
    }

    async fn reset(
        &self,
        tx: &mut Conn,
        machine: Id,
        stream: &Stream,
        reason: ResetReason,
    ) -> Result<(), ApplyError> {
        self.engine
            .reset_stream(tx, machine, stream, reason)
            .await
            .map_err(apply_error)
    }
}

/// An engine over the database and the link, not yet running and not yet
/// registered with the link: what a state that serves requests needs, for
/// the tests of the rest of the API. Growing text is forwarded to nobody.
#[must_use]
pub fn detached(db: &Db, clock: &Arc<dyn Clock>, config: &ServerConfig, link: &Link) -> Engine {
    detached_with(db, clock, config, link, Arc::new(Discard))
}

/// As [`detached`], with growing text forwarded to `live`: the realtime
/// server's sink, for a process that has one.
#[must_use]
pub fn detached_with(
    db: &Db,
    clock: &Arc<dyn Clock>,
    config: &ServerConfig,
    link: &Link,
    live: Arc<dyn LiveSink>,
) -> Engine {
    let engine = Engine::new(EngineParts {
        db: db.clone(),
        clock: Arc::clone(clock),
        config: EngineConfig::from_server(config),
        commands: Arc::new(DispatchCommands::new(link.dispatcher())),
        access: Arc::new(crate::thread_access::AuthzAccess::new(Arc::clone(clock))),
        live,
    });
    // What making a unit for an environment asks of this program: the authority that signs its
    // connector's certificate, the address it dials and the builds it runs.
    engine.attach_host_services(Arc::new(ApiHostServices::new(
        link.certificates().clone(),
        Arc::clone(clock),
        config,
        Arc::new(ReleaseBuilds::of(config)),
    )));
    // What writes a document and a thread as their single exports do, for an archive of a place.
    engine.attach_forms(Arc::new(crate::routes::exports::ApiForms));
    // What keeps the files of a copy until the thread's next turn, takes a delivered copy back out of its
    // workspace, and writes what an agent downloads: the engine's own, over the store attached to it.
    engine.attach_parcels(Arc::new(orch_engine::parcels::WorkspaceParcels::new(
        &engine,
    )));
    engine
}

/// The engine of this process: built over the database and the link,
/// registered as the applier of what machines say, and started. Called once,
/// inside a runtime, before the link has sessions.
#[must_use]
pub fn start(db: &Db, clock: &Arc<dyn Clock>, config: &ServerConfig, link: &Link) -> Engine {
    start_with(db, clock, config, link, Arc::new(Discard))
}

/// As [`start`], with growing text forwarded to `live`.
#[must_use]
pub fn start_with(
    db: &Db,
    clock: &Arc<dyn Clock>,
    config: &ServerConfig,
    link: &Link,
    live: Arc<dyn LiveSink>,
) -> Engine {
    let engine = detached_with(db, clock, config, link, live);
    start_ready(engine, link)
}

/// Starts the production engine only after its persistent content store is attached. Resumed
/// pending inputs must never record an unavailable fallback during asynchronous store startup.
#[must_use]
pub fn start_with_blobs(
    db: &Db,
    clock: &Arc<dyn Clock>,
    config: &ServerConfig,
    link: &Link,
    live: Arc<dyn LiveSink>,
    blobs: Arc<dyn BlobStore>,
) -> Engine {
    let engine = detached_with(db, clock, config, link, live);
    engine.attach_blobs(blobs);
    start_ready(engine, link)
}

fn start_ready(engine: Engine, link: &Link) -> Engine {
    link.set_applier(Arc::new(EngineApplier::new(engine.clone())));
    engine.start();
    engine
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_of_a_commands_row_are_the_stages_the_engine_knows() {
        assert_eq!(stage_of("queued"), Stage::Queued);
        assert_eq!(stage_of("sent"), Stage::Sent);
        assert_eq!(stage_of("accepted"), Stage::Accepted);
        for over in ["succeeded", "failed", "expired", "cancelled"] {
            assert_eq!(stage_of(over), Stage::Over, "{over}");
        }
    }

    #[test]
    fn a_failed_database_is_reported_as_the_databases_and_anything_else_sends_the_events_again() {
        let unreadable = apply_error(EngineError::unreadable("not an item"));
        assert!(matches!(unreadable, ApplyError::Other(text) if text == "not an item"));
        let database = apply_error(EngineError::Db(orch_db::DbError::from(
            sqlx::Error::PoolClosed,
        )));
        assert!(matches!(database, ApplyError::Db(_)));
    }

    #[test]
    fn what_the_dispatcher_cannot_do_is_told_in_the_engines_words() {
        assert!(matches!(
            commands_error(DispatchError::UnknownMachine),
            CommandsError::UnknownMachine
        ));
        assert!(matches!(
            commands_error(DispatchError::Revoked),
            CommandsError::Revoked
        ));
        assert!(matches!(
            commands_error(DispatchError::Stopping),
            CommandsError::Stopping
        ));
        assert!(matches!(
            commands_error(DispatchError::UnknownCommand),
            CommandsError::Other(_)
        ));
    }
}
