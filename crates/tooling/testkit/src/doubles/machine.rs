//! Scripted role commands and events.
//!
//! A scripted machine stands for a connector: it holds a real
//! [`MachineLink`], with a spool and a journal in a directory of its own, and
//! answers the commands it is given as a script says. Nothing is run: a
//! command either gets the result the script names for its kind, or fails with
//! the error it names, or waits until the test lets it go, so that a test can
//! break the link while a command is running. What the machine actually
//! executed is recorded, so that a test can show that a command ran once.

use crate::doubles::link::{Hub, TestIdentity};
use async_trait::async_trait;
use orch_link::commands::{CommandContext, CommandHandler};
use orch_link::machine::{MachineConfig, MachineLink};
use orch_link::spool::SpoolOptions;
use orch_link::tunnels::channel::TunnelOptions;
use orch_link::tunnels::flow::Gauge;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, UnitAfterUpResult,
};
use orch_link_contract::events::Event;
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::streams::Stream;
use orch_types::{Clock, Id};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A command the machine ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    /// The command.
    pub id: Id,
    /// Its kind.
    pub kind: CommandKind,
}

type Script = Box<dyn Fn(&Command) -> Result<CommandResult, CommandError> + Send + Sync>;

/// What answers the commands of a scripted machine.
#[derive(Default)]
pub struct ScriptedHandler {
    executions: Mutex<Vec<Execution>>,
    scripts: Mutex<HashMap<CommandKind, Script>>,
    hold: Mutex<Vec<CommandKind>>,
    started: Notify,
    release: Notify,
    take: Mutex<Duration>,
}

impl std::fmt::Debug for ScriptedHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedHandler").finish_non_exhaustive()
    }
}

impl ScriptedHandler {
    /// Answers commands of `kind` with what `script` makes of them.
    pub fn on(
        &self,
        kind: CommandKind,
        script: impl Fn(&Command) -> Result<CommandResult, CommandError> + Send + Sync + 'static,
    ) {
        lock(&self.scripts).insert(kind, Box::new(script));
    }

    /// Makes commands of `kind` wait until [`ScriptedHandler::release`].
    pub fn hold(&self, kind: CommandKind) {
        lock(&self.hold).push(kind);
    }

    /// Lets the commands that wait go on.
    pub fn release(&self) {
        lock(&self.hold).clear();
        self.release.notify_waiters();
    }

    /// Makes every command take this long, in the time of the runtime.
    pub fn take_time(&self, how_long: Duration) {
        *lock(&self.take) = how_long;
    }

    /// What was run, in the order it started.
    #[must_use]
    pub fn executions(&self) -> Vec<Execution> {
        lock(&self.executions).clone()
    }

    /// How many times a command ran.
    #[must_use]
    pub fn runs_of(&self, id: Id) -> usize {
        lock(&self.executions)
            .iter()
            .filter(|run| run.id == id)
            .count()
    }

    /// Waits until a command has started.
    pub async fn started(&self, id: Id) {
        loop {
            let waiting = self.started.notified();
            if self.runs_of(id) > 0 {
                return;
            }
            waiting.await;
        }
    }
}

#[async_trait]
impl CommandHandler for ScriptedHandler {
    async fn handle(
        &self,
        context: CommandContext,
        command: Command,
    ) -> Result<CommandResult, CommandError> {
        let kind = command.kind();
        lock(&self.executions).push(Execution {
            id: context.id,
            kind,
        });
        self.started.notify_waiters();
        loop {
            let released = self.release.notified();
            if !lock(&self.hold).contains(&kind) {
                break;
            }
            tokio::select! {
                () = released => {}
                () = context.cancel.cancelled() => {
                    return Err(CommandError::new(ErrorCode::Conflict, "cancelled while it ran"));
                }
            }
        }
        let take = *lock(&self.take);
        if !take.is_zero() {
            tokio::time::sleep(take).await;
        }
        let scripts = lock(&self.scripts);
        match scripts.get(&kind) {
            Some(script) => script(&command),
            None => default_result(&command),
        }
    }
}

/// What a command gets when the script says nothing about its kind: the
/// commands that answer with nothing, answer with nothing; a unit command succeeds unless scripted.
fn default_result(command: &Command) -> Result<CommandResult, CommandError> {
    if let Command::UnitAfterUp(body) = command {
        return Ok(CommandResult::UnitAfterUp(UnitAfterUpResult {
            name: body.name.clone(),
            exit: Some(0),
            duration_ms: 0,
            timed_out: false,
            first_lines: String::new(),
        }));
    }
    use CommandKind as K;
    Ok(match command.kind() {
        K::SessionClose => CommandResult::SessionClose(Empty {}),
        K::SessionResync => CommandResult::SessionResync(
            serde_json::from_value(serde_json::json!({ "items": 0 }))
                .map_err(|e| CommandError::new(ErrorCode::Internal, e.to_string()))?,
        ),
        K::TurnInterrupt => CommandResult::TurnInterrupt(Empty {}),
        K::FsRemove => CommandResult::FsRemove(Empty {}),
        K::EnvDetach => CommandResult::EnvDetach(Empty {}),
        K::GitWorktreeCopyPlan => CommandResult::GitWorktreeCopyPlan(
            orch_link_contract::source::GitWorktreeCopyPlanResult {
                independent: false,
                additional_files: 0,
                additional_bytes: 0,
            },
        ),
        other => {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                format!("the script has no answer for {other}"),
            ));
        }
    })
}

/// What a scripted machine is made with.
#[derive(Debug, Clone)]
pub struct MachineSpec {
    /// The roles it has.
    pub roles: Vec<Role>,
    /// The limits of its spool.
    pub spool: SpoolOptions,
    /// Whether a person connected it.
    pub connected: bool,
    /// How its tunnels wait for a new session.
    pub tunnels: TunnelOptions,
    /// A count of the bytes its tunnels hold.
    pub gauge: Option<Arc<Gauge>>,
}

impl Default for MachineSpec {
    fn default() -> Self {
        Self {
            roles: vec![Role::Agent, Role::Environment],
            spool: SpoolOptions {
                max_bytes: 256 * 1024 * 1024,
                max_age: Duration::from_secs(7 * 86_400),
                segment_bytes: 8 * 1024 * 1024,
            },
            connected: true,
            tunnels: TunnelOptions::default(),
            gauge: None,
        }
    }
}

/// A connector's link, in a test.
pub struct ScriptedMachine {
    /// The machine.
    pub machine_id: Id,
    /// Its link.
    pub link: MachineLink,
    /// What answers its commands.
    pub handler: Arc<ScriptedHandler>,
    dir: PathBuf,
    _owned: Option<tempfile::TempDir>,
}

impl std::fmt::Debug for ScriptedMachine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptedMachine")
            .field("machine", &self.machine_id)
            .finish_non_exhaustive()
    }
}

impl ScriptedMachine {
    /// Starts a machine that reaches `hub` showing the certificate of
    /// `identity`, in a directory of its own.
    pub fn start(
        hub: &Hub,
        clock: &Arc<dyn Clock>,
        identity: &TestIdentity,
        spec: &MachineSpec,
    ) -> Self {
        let owned = tempfile::tempdir().unwrap_or_else(|e| panic!("no temporary directory: {e}"));
        let dir = owned.path().to_owned();
        let mut machine = Self::start_in(hub, clock, identity, spec, &dir);
        machine._owned = Some(owned);
        machine
    }

    /// Starts a machine that keeps its spool and journal in `dir`: the same
    /// directory as an earlier run, to see what survives a stop.
    pub fn start_in(
        hub: &Hub,
        clock: &Arc<dyn Clock>,
        identity: &TestIdentity,
        spec: &MachineSpec,
        dir: &Path,
    ) -> Self {
        let link = MachineLink::start(
            MachineConfig {
                hosted_endpoints: None,
                follow_system_endpoints: false,
                machine_id: identity.machine_id,
                connector_version: "0.1.0".to_owned(),
                platform: Platform {
                    os: Os::Linux,
                    arch: Arch::X86_64,
                },
                roles: spec.roles.clone(),
                proof: None,
                endpoints: vec![Hub::endpoint()],
                data_dir: dir.to_owned(),
                spool: spec.spool,
                journal_keep: Duration::from_secs(24 * 3_600),
                connected: spec.connected,
                tunnels: spec.tunnels,
                tunnel_gauge: spec.gauge.clone(),
            },
            hub.dialer(Some(identity.certificate.clone())),
            Arc::clone(clock),
        )
        .unwrap_or_else(|error| panic!("the link could not be started: {error}"));
        let handler = Arc::new(ScriptedHandler::default());
        link.serve(handler.clone());
        Self {
            machine_id: identity.machine_id,
            link,
            handler,
            dir: dir.to_owned(),
            _owned: None,
        }
    }

    /// The directory the machine keeps its spool and journal in.
    #[must_use]
    pub fn data_dir(&self) -> &Path {
        &self.dir
    }

    /// Keeps the temporary directory alive past this machine, for a restart in
    /// the same place.
    pub fn keep_dir(&mut self) -> Option<tempfile::TempDir> {
        self._owned.take()
    }

    /// Publishes an event.
    pub async fn publish(&self, stream: &Stream, event: &Event) {
        self.link
            .publish(stream, event)
            .await
            .unwrap_or_else(|error| panic!("an event could not be published: {error}"));
    }
}
