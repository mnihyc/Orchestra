//! A whole small system for the engine's tests: a real database, a tree of nodes, one agent on one provider in one environment, and a scripted machine behind a hub.
//!
//! Between the machine and the engine stand the two things the API does: it applies
//! what the machine's link delivers inside a transaction that also moves the
//! stream's cursor, and it keeps every command it sends until it ends, so that a
//! "restart" of the API (the engine built anew on the same database, the link made
//! anew from the cursors the database holds) finds what was in flight.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]
#![allow(dead_code, reason = "each test file uses some of what is here")]

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::MissingAccess;
use orch_access_contract::pages as contract_pages;
use orch_api_contract::threads::{ContentPart, Delivery, ItemPayload};
use orch_authz::Caller;
use orch_db::audit::{Actor, Origin};
use orch_db::queries::access::{PageKey, SYSTEM_ID, testing as access_testing};
use orch_db::queries::hierarchy::testing::insert_node;
use orch_db::queries::identity::testing::insert_user;
use orch_db::queries::link as link_queries;
use orch_db::queries::resources::testing::insert_machine;
use orch_db::{Conn, Db};
use orch_engine::access::{AccessFault, Answer, Limit, ProviderUse, ResourceAccess};
use orch_engine::commands::{Commands, CommandsError, Info, Outcome, Outgoing, Stage};
use orch_engine::config::EngineConfig;
use orch_engine::inputs::InputRequest;
use orch_engine::live::{LiveDelta, LiveSink};
use orch_engine::{Engine, EngineParts};
use orch_link::commands::{CommandHandle, Progress};
use orch_link::controller::{ControllerLink, Notice};
use orch_link_contract::commands::{
    AgentStartResult, Cmd, Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode,
    SessionMode, SessionOpenResult, TurnStart,
};
use orch_link_contract::events::{
    DeltaField, EmittedItemKind, Event, ItemCompleted, ItemDelta, ItemStarted, TurnCompleted,
    TurnOutcome, TurnStarted,
};
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_settings::{Target, write as settings_write};
use orch_testkit::database::TestDb;
use orch_testkit::doubles::link::{Hub, MachineRecord, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, Json, SystemClock, Time, new_id};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How long a test waits for something to happen before it fails.
pub const PATIENCE: Duration = Duration::from_secs(25);

/// The model the tests' agent runs.
pub const MODEL: &str = "fake-1";

/// Runs one real case on default stacks even when the enclosing proof enlarges its own stacks.
pub fn on_default_stack(case: &str) {
    let result = std::process::Command::new(std::env::current_exe().unwrap())
        .args([case, "--exact", "--test-threads=1"])
        .env_remove("RUST_MIN_STACK")
        .output()
        .unwrap();
    let one_case_ran = String::from_utf8_lossy(&result.stdout)
        .contains("test result: ok. 1 passed; 0 failed; 0 ignored;");
    assert!(
        result.status.success() && one_case_ran,
        "{case} must pass on default stacks: status {:?}, one case ran: {one_case_ran}",
        result.status
    );
}

/// Waits until `check` holds, looking again every few milliseconds.
pub async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let started = Instant::now();
    loop {
        if check().await {
            return;
        }
        assert!(started.elapsed() < PATIENCE, "gave up waiting for: {what}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Samples continued absence after an observed transition; positive progress must use an event or predicate.
pub async fn meanwhile(how_long: Duration) {
    tokio::time::sleep(how_long).await;
}

// ---------------------------------------------------------------- the access double

/// Answers the engine's questions about resources: everybody may, but those who were told they may not.
#[derive(Default)]
pub struct TestAccess {
    denied: Mutex<HashSet<Id>>,
    no_bypass: Mutex<HashSet<Id>>,
    unreachable: AtomicBool,
}

impl TestAccess {
    /// From now on `person` may not use anything.
    pub fn deny(&self, person: Id) {
        lock(&self.denied).insert(person);
    }

    /// From now on `person` may again.
    pub fn allow(&self, person: Id) {
        lock(&self.denied).remove(&person);
    }

    /// From now on `person` may not let a turn past a limit.
    pub fn refuse_bypass(&self, person: Id) {
        lock(&self.no_bypass).insert(person);
    }

    /// From now on no question can be answered.
    pub fn disconnect(&self) {
        self.unreachable.store(true, Ordering::SeqCst);
    }

    /// Questions can be answered again.
    pub fn reconnect(&self) {
        self.unreachable.store(false, Ordering::SeqCst);
    }

    fn answer(&self, person: Id) -> Result<Answer, AccessFault> {
        if self.unreachable.load(Ordering::SeqCst) {
            return Err(AccessFault::NotConnected);
        }
        Ok(if lock(&self.denied).contains(&person) {
            Answer::Refused(Vec::<MissingAccess>::new())
        } else {
            Answer::Allowed { arrangements: None }
        })
    }
}

#[async_trait]
impl ResourceAccess for TestAccess {
    async fn provider_use(
        &self,
        _: &mut Conn,
        ask: &ProviderUse<'_>,
    ) -> Result<Answer, AccessFault> {
        self.answer(ask.person)
    }

    async fn agent_use(
        &self,
        _: &mut Conn,
        person: Id,
        _: Id,
        _: Id,
    ) -> Result<Answer, AccessFault> {
        self.answer(person)
    }

    async fn environment_manage(
        &self,
        _: &mut Conn,
        person: Id,
        _: Id,
    ) -> Result<Answer, AccessFault> {
        self.answer(person)
    }

    async fn may_pass(
        &self,
        _: &mut Conn,
        person: Id,
        _: bool,
        _: &Limit,
    ) -> Result<bool, AccessFault> {
        if self.unreachable.load(Ordering::SeqCst) {
            return Err(AccessFault::NotConnected);
        }
        Ok(!lock(&self.denied).contains(&person) && !lock(&self.no_bypass).contains(&person))
    }
}

// ---------------------------------------------------------------- the live sink double

/// Records what the engine forwards at once.
#[derive(Default)]
pub struct Recorder {
    watched_jobs: Mutex<HashSet<Id>>,
    deltas: Mutex<Vec<LiveDelta>>,
    approvals: Mutex<Vec<Vec<Id>>>,
    open: Mutex<HashMap<Id, usize>>,
    watching: Mutex<HashSet<(Id, Id)>>,
}

impl Recorder {
    /// A plan is actively following a count even while its machine is silent.
    pub fn watching_job(&self, job: Id, visible: bool) {
        if visible {
            lock(&self.watched_jobs).insert(job);
        } else {
            lock(&self.watched_jobs).remove(&job);
        }
    }
    /// A person's thread is really visible, independently of owning or following it.
    pub fn watching(&self, thread: Id, person: Id, visible: bool) {
        let mut watching = lock(&self.watching);
        if visible {
            watching.insert((thread, person));
        } else {
            watching.remove(&(thread, person));
        }
    }

    /// Says how many people have a document open now.
    pub fn open_by(&self, document: Id, people: usize) {
        lock(&self.open).insert(document, people);
    }

    pub fn deltas(&self) -> Vec<LiveDelta> {
        lock(&self.deltas).clone()
    }

    pub fn approvals(&self) -> Vec<Vec<Id>> {
        lock(&self.approvals).clone()
    }
}

impl LiveSink for Recorder {
    fn job_is_watched(&self, job: Id) -> bool {
        lock(&self.watched_jobs).contains(&job)
    }
    fn delta(&self, delta: &LiveDelta) {
        lock(&self.deltas).push(delta.clone());
    }

    fn approvals_changed(&self, deciders: &[Id]) {
        lock(&self.approvals).push(deciders.to_vec());
    }

    fn document_readers(&self, document: Id) -> usize {
        lock(&self.open).get(&document).copied().unwrap_or(0)
    }

    fn thread_is_visible(&self, thread: Id, person: Id) -> bool {
        lock(&self.watching).contains(&(thread, person))
    }
}

// ---------------------------------------------------------------- the commands double

struct Issued {
    cmd: Cmd,
    machine: Id,
    handle: CommandHandle,
    at: DateTime<Utc>,
}

/// The API's end of a machine's commands: each is kept from the moment it is issued, a command asked for
/// twice under one identifier is made once, and when the API "restarts" the ones that did not end are
/// handed to the new link, as the API's rows would be.
pub struct ScriptedCommands {
    primary: Id,
    link: Mutex<ControllerLink>,
    others: Mutex<HashMap<Id, ControllerLink>>,
    issued: Mutex<HashMap<Id, Issued>>,
    clock: Arc<dyn Clock>,
    process: AtomicU64,
    /// Whether the link says that it holds events of a machine not taken in yet ([`World::hearing_behind`]).
    behind: AtomicBool,
    stopped: AtomicBool,
    /// Weak ownership witnesses for every API generation and retained command-only worker.
    processes: Mutex<Vec<Weak<()>>>,
}

impl ScriptedCommands {
    fn new(primary: Id, link: ControllerLink, clock: Arc<dyn Clock>) -> Self {
        Self {
            primary,
            link: Mutex::new(link),
            others: Mutex::new(HashMap::new()),
            issued: Mutex::new(HashMap::new()),
            clock,
            process: AtomicU64::new(1),
            behind: AtomicBool::new(false),
            stopped: AtomicBool::new(false),
            processes: Mutex::new(Vec::new()),
        }
    }

    /// The link of a machine: the first one the world has, or one added to it.
    fn link_of(&self, machine: Id) -> ControllerLink {
        if machine == self.primary {
            lock(&self.link).clone()
        } else {
            lock(&self.others)
                .get(&machine)
                .cloned()
                .unwrap_or_else(|| panic!("the world has no machine {machine}"))
        }
    }

    /// The API restarted: the commands that did not end are handed to the new link, and the engines of
    /// the processes before it can no longer reach the machine.
    fn restart(&self, link: &ControllerLink) {
        self.process.fetch_add(1, Ordering::SeqCst);
        let mut issued = lock(&self.issued);
        for entry in issued.values_mut() {
            // Only the commands of the machine whose link was made anew are handed to the new link;
            // the links of the others stand.
            if entry.machine == self.primary && !entry.handle.progress().is_final() {
                entry.handle = link.commands().adopt(entry.cmd.clone(), Progress::Queued);
            }
        }
        *lock(&self.link) = link.clone();
    }

    /// The commands asked for as the engine of the current process.
    fn current(self: &Arc<Self>) -> Arc<Process> {
        let lifetime = Arc::new(());
        lock(&self.processes).push(Arc::downgrade(&lifetime));
        Arc::new(Process {
            commands: Arc::clone(self),
            number: self.process.load(Ordering::SeqCst),
            _lifetime: lifetime,
        })
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.process.fetch_add(1, Ordering::SeqCst);
    }

    fn live_processes(&self) -> usize {
        lock(&self.processes)
            .iter()
            .filter(|process| process.strong_count() > 0)
            .count()
    }

    /// The commands as the engine of the current process asks for them, for a test that sends one as the
    /// engine would have.
    pub fn as_engine(self: &Arc<Self>) -> Arc<dyn Commands> {
        self.current()
    }

    /// Drives the controller issuer as the gateway does. Advancing a manual
    /// domain clock alone does not run this link's command-expiry sweep.
    pub fn tick(&self, machine: Id) {
        self.link_of(machine)
            .commands()
            .tick(tokio::time::Instant::now());
    }

    /// Actual controller progress, also for an observation that has no durable issued row.
    pub fn native_progress(&self, machine: Id, command: Id) -> Option<Progress> {
        self.link_of(machine).commands().progress(command)
    }

    /// The commands issued, in no order, with their kinds.
    pub fn kinds(&self) -> Vec<(Id, String)> {
        lock(&self.issued)
            .values()
            .map(|entry| (entry.cmd.id, entry.cmd.kind.clone()))
            .collect()
    }

    /// The real controller's acknowledgement progress, including reports discarded without a DB cursor.
    pub fn acknowledged(&self, machine: Id, stream: &Stream) -> i64 {
        self.link_of(machine)
            .cursors()
            .get(&stream.to_string())
            .copied()
            .unwrap_or(0)
    }

    /// How many commands of a kind were issued.
    pub fn issued_of(&self, kind: &str) -> usize {
        lock(&self.issued)
            .values()
            .filter(|entry| entry.cmd.kind == kind)
            .count()
    }

    /// The commands of a kind as they were issued (a secret still the reference to it, a ticket still the
    /// request for one), with the machine each was for.
    pub fn issued_bodies(&self, kind: &str) -> Vec<(Id, Json)> {
        lock(&self.issued)
            .values()
            .filter(|entry| entry.cmd.kind == kind)
            .map(|entry| (entry.machine, entry.cmd.body.clone()))
            .collect()
    }

    /// Frozen transport commands, including their original identities, keys and deadlines.
    pub fn issued_commands(&self, kind: &str) -> Vec<(Id, Cmd)> {
        lock(&self.issued)
            .values()
            .filter(|entry| entry.cmd.kind == kind)
            .map(|entry| (entry.machine, entry.cmd.clone()))
            .collect()
    }

    fn handle(&self, id: Id) -> Option<CommandHandle> {
        lock(&self.issued)
            .get(&id)
            .map(|entry| entry.handle.clone())
    }
}

/// The commands of one process of the API: once the API has restarted, the ones before it reach nothing.
struct Process {
    commands: Arc<ScriptedCommands>,
    number: u64,
    _lifetime: Arc<()>,
}

impl Process {
    fn alive(&self) -> Result<(), CommandsError> {
        if !self.commands.stopped.load(Ordering::SeqCst)
            && self.commands.process.load(Ordering::SeqCst) == self.number
        {
            Ok(())
        } else {
            Err(CommandsError::Stopping)
        }
    }

    async fn wait_handle(&self, mut handle: CommandHandle) -> Result<Outcome, CommandsError> {
        let waiting = handle.wait();
        tokio::pin!(waiting);
        loop {
            tokio::select! {
                progress = &mut waiting => {
                    self.alive()?;
                    return outcome_of(&progress).ok_or(CommandsError::Stopping);
                }
                () = tokio::time::sleep(Duration::from_millis(50)) => self.alive()?,
            }
        }
    }
}

fn stage_of(progress: &Progress) -> Stage {
    match progress {
        Progress::Queued => Stage::Queued,
        Progress::Sent => Stage::Sent,
        Progress::Accepted => Stage::Accepted,
        _ => Stage::Over,
    }
}

fn outcome_of(progress: &Progress) -> Option<Outcome> {
    match progress {
        Progress::Succeeded(body) => Some(Outcome::Succeeded(body.clone())),
        Progress::Failed(error) => Some(Outcome::Failed(error.clone())),
        Progress::Expired => Some(Outcome::Expired),
        Progress::Cancelled => Some(Outcome::Cancelled),
        _ => None,
    }
}

#[async_trait]
impl Commands for Process {
    async fn retained_body(&self, id: Id) -> Result<Option<Json>, CommandsError> {
        Ok(lock(&self.commands.issued)
            .get(&id)
            .map(|issued| issued.cmd.body.clone()))
    }
    async fn retained_command(
        &self,
        id: Id,
    ) -> Result<Option<orch_engine::commands::RetainedCommand>, CommandsError> {
        Ok(lock(&self.commands.issued).get(&id).map(|issued| {
            orch_engine::commands::RetainedCommand {
                machine: issued.machine,
                body: issued.cmd.body.clone(),
            }
        }))
    }
    async fn observe(&self, command: Outgoing) -> Result<Outcome, CommandsError> {
        self.alive()?;
        let cmd = Cmd {
            id: command.id,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline: Time::new(
                self.commands.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            ),
            body: command.body,
        };
        let handle = self.commands.link_of(command.machine).commands().issue(cmd);
        self.wait_handle(handle).await
    }

    async fn issue(&self, command: Outgoing) -> Result<(), CommandsError> {
        self.alive()?;
        let mut issued = lock(&self.commands.issued);
        if issued.contains_key(&command.id) {
            return Ok(());
        }
        let now = self.commands.clock.now();
        let deadline = command.deadline.unwrap_or_else(|| {
            Time::new(now + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())))
        });
        let cmd = Cmd {
            id: command.id,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline,
            body: command.body,
        };
        let handle = self
            .commands
            .link_of(command.machine)
            .commands()
            .issue(cmd.clone());
        issued.insert(
            command.id,
            Issued {
                cmd,
                machine: command.machine,
                handle,
                at: now,
            },
        );
        Ok(())
    }

    async fn status(&self, id: Id) -> Result<Option<Info>, CommandsError> {
        self.alive()?;
        Ok(lock(&self.commands.issued).get(&id).map(|entry| {
            let progress = entry.handle.progress();
            Info {
                stage: stage_of(&progress),
                issued_at: entry.at,
                outcome: outcome_of(&progress),
            }
        }))
    }

    async fn wait(&self, id: Id) -> Result<Outcome, CommandsError> {
        self.alive()?;
        let Some(handle) = self.commands.handle(id) else {
            return Err(CommandsError::Other("there is no such command".to_owned()));
        };
        self.wait_handle(handle).await
    }

    async fn cancel(&self, id: Id) -> Result<bool, CommandsError> {
        self.alive()?;
        let (machine, open) = {
            let issued = lock(&self.commands.issued);
            match issued.get(&id) {
                Some(entry) => (Some(entry.machine), !entry.handle.progress().is_final()),
                None => (None, false),
            }
        };
        if let Some(machine) = machine {
            self.commands.link_of(machine).commands().cancel(id);
        }
        Ok(open)
    }

    async fn stopped(&self, id: Id) -> Result<bool, CommandsError> {
        let machine = lock(&self.commands.issued)
            .get(&id)
            .map(|entry| entry.machine);
        Ok(machine.is_none_or(|machine| self.commands.link_of(machine).commands().answered(id)))
    }

    fn hears_behind(&self, _machine: Id, _stream: &Stream) -> bool {
        self.commands.behind.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------- the harness' answers

/// A descriptor of the tests' agent.
pub fn descriptor() -> Json {
    json!({
        "kind": "fake", "adapter_version": 1,
        "harness": { "name": "Fake", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
        "arrangements": [
            { "type": "together" },
            { "type": "separated", "executor": "orch-exec", "executor_version": "*", "tool_sets": ["plain"] },
            { "type": "key_separated" }
        ],
        "native_provider": { "harness_kind": "fake", "title": "Fake subscription", "arrangements": ["separated"] },
        "forwarded_protocols": ["openai-responses"],
        "approval_modes": ["ask", "on_request", "bypass"],
        "sandbox_modes": ["read_only", "workspace", "open"],
        "side_thread_modes": ["read_only"],
        "capabilities": {
            "steer": true, "interrupt": true, "approvals": ["command", "file_change", "user_input"],
            "input_images": true, "usage_tokens": true, "memory": "none"
        },
        "parameters": []
    })
}

/// The failures the scripted machine is to answer the next commands of a kind with.
pub type Refusals = Mutex<HashMap<CommandKind, VecDeque<CommandError>>>;

/// The commands the scripted machine ran, in the order it ran them.
#[derive(Default)]
pub struct Seen {
    commands: Mutex<Vec<Command>>,
}

impl Seen {
    pub fn all(&self) -> Vec<Command> {
        lock(&self.commands).clone()
    }

    pub fn of_kind(&self, kind: CommandKind) -> Vec<Command> {
        self.all()
            .into_iter()
            .filter(|command| command.kind() == kind)
            .collect()
    }
}

/// The tests' engine configuration: quick schedules and deployed hosting response bounds.
pub fn config() -> EngineConfig {
    EngineConfig {
        interrupt_force: Duration::from_millis(1_500),
        start_timeout: Duration::from_millis(6_000),
        sweep_every: Duration::from_millis(40),
        recheck_every: Duration::from_millis(250),
        post_commit_grace: Duration::from_millis(5),
        sweep_batch: 200,
        // Growing text is held as a deployment holds it, for a tenth of its interval.
        stream_flush: Duration::from_millis(100),
        hosting: orch_engine::hosting::HostingConfig {
            // As long as a deployment's: a runner renews its lease every third of it, and on a busy machine
            // a test's runner can be kept from doing so for a couple of seconds; the job would then be taken
            // up a second time while the first runner is still at it.
            job_lease: orch_engine::hosting::JOB_LEASE,
            enroll_timeout: orch_engine::hosting::HostingConfig::default().enroll_timeout,
            reaper_every: Duration::from_millis(150),
            removal_first_pause: Duration::from_millis(300),
            reaper_retry_max: Duration::from_secs(2),
            removal_notice: Duration::from_secs(3_600),
            stop_wait: orch_engine::hosting::STOP_WAIT,
            attach_wait: orch_engine::hosting::ATTACH_WAIT,
            attach_retry: Duration::from_millis(400),
            login_pending: Duration::from_secs(4_500),
        },
        cap_check: Duration::ZERO,
        soft_delete: Duration::from_secs(30 * 86_400),
        manage_plan_kept: orch_engine::config::MANAGE_PLAN_KEPT,
        ..EngineConfig::default()
    }
}

// ---------------------------------------------------------------- requests and events

/// What a turn's text says after the block of where the agent is (T3), which a session's first input, and an input
/// after a change of it, begin with: the words the test sent, as they reach the agent.
#[must_use]
pub fn words_after_where(text: &str) -> &str {
    text.strip_prefix("<orch>\nWhere you are\n")
        .and_then(|rest| rest.split_once("\n</orch>\n\n"))
        .map_or(text, |(_, after)| after)
}

/// An input that waits its turn.
pub fn queued(text: &str) -> InputRequest {
    request(text, Delivery::Queue)
}

/// An input that goes into the turn that runs.
pub fn steering(text: &str) -> InputRequest {
    request(text, Delivery::Steer)
}

/// An input that stops the turn that runs and goes first.
pub fn interrupting(text: &str) -> InputRequest {
    request(text, Delivery::Interrupt)
}

fn request(text: &str, delivery: Delivery) -> InputRequest {
    InputRequest {
        plan: false,
        content: vec![ContentPart::Text {
            text: text.to_owned(),
        }],
        delivery,
        client_id: new_id().to_string(),
        bypass: None,
    }
}

/// The harness began the turn.
pub fn turn_began(turn: Id) -> Event {
    Event::TurnStarted(TurnStarted {
        turn_id: turn,
        native_turn_ref: Some(format!("native-{turn}")),
    })
}

/// The agent began a message.
pub fn message_began(turn: Id, native_ref: &str) -> Event {
    Event::ItemStarted(ItemStarted {
        turn_id: turn,
        native_ref: native_ref.to_owned(),
        stream: "main".to_owned(),
        parent_native_ref: None,
        kind: EmittedItemKind::AgentMessage,
        payload: orch_link_contract::foreign::ItemPayload::AgentMessage(
            orch_agent_contract::events::AgentMessagePayload {
                text: String::new(),
                text_len: 0,
                phase: Some(orch_agent_contract::events::AgentMessagePhase::Final),
            },
        ),
    })
}

/// The message grew by `data`, which came after `before` characters.
pub fn message_grew(native_ref: &str, before: i64, data: &str) -> Event {
    Event::ItemDelta(ItemDelta {
        native_ref: native_ref.to_owned(),
        field: DeltaField::Text,
        offset: before,
        len: i64::try_from(data.chars().count()).unwrap_or(i64::MAX),
        data: data.to_owned(),
    })
}

/// The message is final.
pub fn message_done(native_ref: &str, text: &str) -> Event {
    Event::ItemCompleted(ItemCompleted {
        native_ref: native_ref.to_owned(),
        status: orch_agent_contract::events::ItemStatus::Completed,
        payload: orch_link_contract::foreign::ItemPayload::AgentMessage(
            orch_agent_contract::events::AgentMessagePayload {
                text: text.to_owned(),
                text_len: i64::try_from(text.chars().count()).unwrap_or(i64::MAX),
                phase: Some(orch_agent_contract::events::AgentMessagePhase::Final),
            },
        ),
        output_blob: None,
    })
}

/// The turn ended as `outcome`.
pub fn turn_ended(turn: Id, outcome: TurnOutcome) -> Event {
    Event::TurnCompleted(TurnCompleted {
        turn_id: turn,
        status: outcome,
        error: None,
    })
}

/// The harness asks whether a command may run.
pub fn command_asked(turn: Id, request_ref: &str) -> Event {
    Event::ApprovalRequested(orch_link_contract::events::ApprovalRequested {
        turn_id: turn,
        request_ref: request_ref.to_owned(),
        title: "Run a command".to_owned(),
        request: serde_json::from_value(json!({
            "request_kind": "command",
            "detail": { "command": "rm -rf build", "cwd": "/workspace/web", "reason": null },
            "options": [
                { "id": "allow_once", "label": "Allow once", "style": "allow", "scope": "once" },
                { "id": "deny", "label": "Deny", "style": "deny", "scope": null }
            ]
        }))
        .unwrap(),
    })
}

/// What was said in a thread, in the order it was made: `user: ...` and `agent: ...`.
pub fn said(sync: &orch_engine::views::ThreadSyncData) -> Vec<String> {
    sync.items
        .iter()
        .filter_map(|item| match &item.payload {
            ItemPayload::UserMessage(message) => Some(format!(
                "user: {}",
                message
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        ContentPart::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            ItemPayload::AgentMessage(message) => Some(format!("agent: {}", message.text)),
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------- the world

/// A machine added to the world beside the first.
pub struct Added {
    /// The machine.
    pub id: Id,
    /// Its scripted link.
    pub machine: Arc<ScriptedMachine>,
    /// What it was asked.
    pub seen: Arc<Seen>,
}

/// Everything a test stands on.
pub struct World {
    pub pool: Db,
    pub clock: Arc<dyn Clock>,
    pub hub: Hub,
    pub machine: Arc<ScriptedMachine>,
    pub machine_id: Id,
    pub commands: Arc<ScriptedCommands>,
    pub access: Arc<TestAccess>,
    pub live: Arc<Recorder>,
    pub seen: Arc<Seen>,
    /// Every command that any machine of the world ran, in the order they started, with the machine.
    pub ordered: Arc<Mutex<Vec<(Id, Command)>>>,
    /// What the harness of the world's agent declares at every `agent.start` ([`World::declares`]).
    declared: Arc<Mutex<Json>>,
    pki: TestPki,
    refusals: Arc<Refusals>,
    engine: Mutex<Option<Engine>>,
    applier_tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    machines: Mutex<Vec<Weak<ScriptedMachine>>>,
    config: Mutex<EngineConfig>,
    gate: Arc<tokio::sync::Mutex<()>>,
    appliers: Arc<AtomicU64>,
    /// How many words about a machine's presence (a session up, a session down) the appliers have written
    /// to the database and told the engine of, by machine.
    presence_words: Arc<Mutex<HashMap<Id, u64>>>,
    pub ada: Id,
    pub root: Id,
    pub project: Id,
    pub topic: Id,
    pub agent: Id,
    pub provider: Id,
    pub environment: Id,
    /// Drop the owned database after the fixture's service handles, never before them.
    pub db: TestDb,
}

/// What a world is made of beyond what every world has.
#[derive(Debug, Clone, Default)]
pub struct Setup {
    /// The domain clock, held by tests that exercise monotonic hosting waits.
    pub clock: Option<Arc<dyn Clock>>,
    /// The provider is an API provider on a key that the server holds, with this key as its text, run on the
    /// machine; without, it is a subscription whose login the harness holds.
    pub key: Option<&'static str>,
    /// Changes selected engine timers for a test that deliberately expires a wait.
    pub tune: Option<fn(&mut EngineConfig)>,
    /// The subscription was looked at just before the world began, as a key provider is: the background
    /// reader keeps its ordinary cadence and starts no harness of its own while a test counts what a turn
    /// starts.
    pub observed: bool,
}

impl Drop for World {
    fn drop(&mut self) {
        // Async shutdown joins these tasks before successful fixture removal. During an
        // assertion unwind, fence/abort them before the last field removes the database.
        if let Some(engine) = lock(&self.engine).as_ref() {
            engine.stop();
        }
        self.commands.stop();
        self.appliers.fetch_add(1, Ordering::SeqCst);
        for task in lock(&self.applier_tasks).iter() {
            task.abort();
        }
    }
}

impl World {
    /// A world with one topic, one agent on one provider, one environment, all on one machine that is online.
    pub async fn new() -> Self {
        Self::build(Setup::default()).await
    }

    /// As [`World::new`], with an API provider on a key the server holds: the key is `key`, sealed in the
    /// store, and the provider is run on the machine. A recent observation attempt keeps the background
    /// reader on its ordinary cadence while a test controls the turn's startup and native refusals.
    pub async fn on_a_key(key: &'static str) -> Self {
        Self::build(Setup {
            key: Some(key),
            ..Setup::default()
        })
        .await
    }

    /// As [`World::new`], with selected timers changed for a test that expires a wait
    /// or requires its deployment bound.
    pub async fn tuned(change: fn(&mut EngineConfig)) -> Self {
        Self::build(Setup {
            tune: Some(change),
            ..Setup::default()
        })
        .await
    }

    /// As [`World::new`], with the subscription observed just before the world began: its first scheduled read
    /// does not start its harness while a test counts what a turn starts.
    pub async fn observed() -> Self {
        Self::build(Setup {
            observed: true,
            ..Setup::default()
        })
        .await
    }

    /// A pin and its advertised program present before startup; the scripted program still reports 1.0.0.
    pub async fn pinned(version: &str) -> Self {
        Self::build_profile(
            Setup {
                observed: true,
                ..Setup::default()
            },
            json!({"harness_version":version}),
        )
        .await
    }

    /// A world whose domain clock stays under the test's control.
    pub async fn with_clock(clock: Arc<dyn Clock>) -> Self {
        Self::build(Setup {
            clock: Some(clock),
            ..Setup::default()
        })
        .await
    }

    async fn build(setup: Setup) -> Self {
        Self::build_profile(setup, json!({})).await
    }

    async fn build_profile(setup: Setup, profile: Json) -> Self {
        let advertised_version = profile
            .get("harness_version")
            .and_then(Json::as_str)
            .unwrap_or("1.0.0")
            .to_owned();
        let db = TestDb::fresh().await;
        let pool = Db::from_pool(
            PgPoolOptions::new()
                .max_connections(16)
                .connect(&db.url)
                .await
                .expect("a pool of its own"),
        );
        let clock: Arc<dyn Clock> = setup.clock.unwrap_or_else(|| Arc::new(SystemClock));
        let now = clock.now();
        let mut conn = pool.acquire().await.unwrap();
        let ada = new_id();
        insert_user(&mut conn, ada, "ada").await.unwrap();
        let (root, project, topic) = (new_id(), new_id(), new_id());
        insert_node(&mut conn, root, "org", None, "Acme", ada)
            .await
            .unwrap();
        access_testing::insert_member(&mut conn, root, ada, root)
            .await
            .unwrap();
        insert_node(&mut conn, project, "project", Some(root), "Web", ada)
            .await
            .unwrap();
        insert_node(&mut conn, topic, "topic", Some(project), "Login", ada)
            .await
            .unwrap();
        for node in [root, project, topic] {
            access_testing::name_owner(&mut conn, node, ("user", ada), now)
                .await
                .unwrap();
        }
        let page = PageKey::new("system", SYSTEM_ID, "defaults");
        for shipped in contract_pages::shipped() {
            access_testing::add_entry(
                &mut conn,
                &page,
                shipped.profile.as_str(),
                shipped.permission.as_str(),
                ("role", None, Some(shipped.role.as_str())),
                None,
                now,
            )
            .await
            .unwrap();
        }
        let machine_id = insert_machine(&mut conn, "m1", "connected").await.unwrap();
        sqlx::query("UPDATE machines SET roles = ARRAY['agent','environment','provider'], online = true, facts = $2, last_seen_at = clock_timestamp() WHERE id = $1")
            .bind(machine_id)
            .bind(json!({
                "adapters": [{"kind":"fake","adapter_version":1,"builtin":true,"default_home_owned":true}],
                "harnesses": [{"kind":"fake","path":"/test/harness","version":advertised_version,"daemon":false}],
                "policy": {"agent.harnesses":[{"kind":"fake","path":"/test/harness"}],
                    "provider.arrangements":["separated","key_separated","credential_shared","handed_over"]}
            }))
            .execute(&mut *conn)
            .await
            .unwrap();
        let (agent, provider, environment, template) = (new_id(), new_id(), new_id(), new_id());
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Fake agent', $2, $3, $3, 'fake', $4, $5)",
        )
        .bind(agent)
        .bind(ada)
        .bind(root)
        .bind(profile)
        .bind(descriptor())
        .execute(&mut *conn)
        .await
        .unwrap();
        if let Some(key) = setup.key {
            insert_api_provider(&mut conn, provider, ada, root, machine_id, key, now).await;
        } else {
            sqlx::query(
                "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                        machine_id, arrangements, status, facts_requested_at)
                 VALUES ($1, 'Fake subscription', $2, $3, $3, 'subscription', 'fake', $4, $5,
                         ARRAY['separated'], 'ready', $6)",
            )
            .bind(provider)
            .bind(ada)
            .bind(root)
            .bind(agent)
            .bind(machine_id)
            .bind(setup.observed.then_some(now))
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Fake One', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(provider)
        .bind(MODEL)
        .bind(json!({ "efforts": ["low", "high"], "default_effort": "low", "input_images": true }))
        .bind(ada)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query("INSERT INTO env_templates (id, name, driver, spec) VALUES ($1, 'template', 'test', '{}')")
            .bind(template)
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                       template_id, generation, status, facts)
             VALUES ($1, 'Workspace', $2, $3, 'managed', $4, '/workspace', $5, $6, 1, 'running', $7)",
        )
        .bind(environment)
        .bind(ada)
        .bind(root)
        .bind(topic)
        .bind(machine_id)
        .bind(template)
        .bind(json!({"executors":[{"kind":"orch-exec","available":true,"sandbox_modes":["read_only","workspace","open"],"tool_sets":["plain"]}]}))
        .execute(&mut *conn)
        .await
        .unwrap();
        // Creation names the first owner in Owners; owner_id alone is attribution.
        for (kind, id) in [("provider", provider), ("environment", environment)] {
            access_testing::add_entry(
                &mut conn,
                &PageKey::new(kind, id, "live"),
                "any",
                "access.own",
                ("user", Some(ada), None),
                None,
                now,
            )
            .await
            .unwrap();
        }
        drop(conn);

        // The machine and the hub between it and the system.
        let pki = TestPki::new(Arc::clone(&clock));
        let identity = pki.machine(machine_id);
        let hub = Hub::new(Arc::clone(&clock), pki.certificate());
        let roles = vec![Role::Agent, Role::Environment, Role::Provider];
        let (link, notices) = hub.add_machine(
            machine_id,
            roles.clone(),
            Default::default(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        let machine = Arc::new(ScriptedMachine::start(
            &hub,
            &clock,
            &identity,
            &MachineSpec {
                roles,
                ..MachineSpec::default()
            },
        ));
        let commands = Arc::new(ScriptedCommands::new(
            machine_id,
            link.clone(),
            Arc::clone(&clock),
        ));
        let access = Arc::new(TestAccess::default());
        let live = Arc::new(Recorder::default());
        let seen = Arc::new(Seen::default());
        let refusals = Arc::new(Refusals::default());
        let mut waits = config();
        if let Some(tune) = setup.tune {
            tune(&mut waits);
        }
        let engine = build_engine(&pool, &clock, &waits, &commands, &access, &live);
        engine.start();
        let world = Self {
            db,
            pool,
            clock,
            hub,
            machine,
            machine_id,
            commands,
            access,
            live,
            seen,
            ordered: Arc::new(Mutex::new(Vec::new())),
            declared: Arc::new(Mutex::new(descriptor())),
            pki,
            refusals,
            engine: Mutex::new(Some(engine)),
            applier_tasks: Mutex::new(Vec::new()),
            machines: Mutex::new(Vec::new()),
            config: Mutex::new(waits),
            gate: Arc::new(tokio::sync::Mutex::new(())),
            appliers: Arc::new(AtomicU64::new(0)),
            presence_words: Arc::new(Mutex::new(HashMap::new())),
            ada,
            root,
            project,
            topic,
            agent,
            provider,
            environment,
        };
        lock(&world.machines).push(Arc::downgrade(&world.machine));
        world.answer_commands();
        world.spawn_applier(link, notices);
        let hub = world.hub.clone();
        eventually("the machine connects", || {
            let up = hub.session_of(machine_id).is_some();
            async move { up }
        })
        .await;
        // The word that the machine is here is written by the applier when it gets to it: a test that
        // sets the machine's presence by hand must not be overtaken by it.
        world.wait_for_presence_words(machine_id, 1).await;
        world
    }

    /// Waits until the applier has written `words` words about the presence of `machine` to the database
    /// and told the engine of them. The hub says a machine is here as soon as its session stands; the
    /// applier writes that to the database whenever it gets to it, which on a busy machine can be after a
    /// test has begun to set the presence by hand: the machine would then be online again, in a session
    /// the test did not make.
    pub async fn wait_for_presence_words(&self, machine: Id, words: u64) {
        eventually(
            &format!("{words} word(s) about the presence of {machine} to be written"),
            || async move {
                lock(&self.presence_words)
                    .get(&machine)
                    .copied()
                    .unwrap_or(0)
                    >= words
            },
        )
        .await;
    }

    /// The engine as it is now.
    pub fn engine(&self) -> Engine {
        lock(&self.engine)
            .as_ref()
            .expect("the fixture's engine is still running")
            .clone()
    }

    /// End the fixture while its runtime can still poll rollbacks and connection cleanup.
    /// Every engine generation owns a Process witness; appliers separately own database clones.
    pub async fn shutdown(&self) {
        let started = Instant::now();
        if let Some(engine) = lock(&self.engine).take() {
            engine.stop();
        }
        self.commands.stop();
        self.appliers.fetch_add(1, Ordering::SeqCst);
        let tasks = std::mem::take(&mut *lock(&self.applier_tasks));
        eprintln!(
            "fixture shutdown phase=joining-appliers tasks={} elapsed={:?}",
            tasks.len(),
            started.elapsed()
        );
        for task in &tasks {
            task.abort();
        }
        let mut failures = Vec::new();
        for task in tasks {
            if let Err(error) = task.await
                && !error.is_cancelled()
            {
                failures.push(error.to_string());
            }
        }
        eprintln!(
            "fixture shutdown phase=draining-generations owners={} elapsed={:?}",
            self.commands.live_processes(),
            started.elapsed()
        );
        eventually(
            "all fixture API generations and their background workers release ownership",
            || {
                let alive = self.commands.live_processes();
                async move { alive == 0 }
            },
        )
        .await;
        let machines: Vec<_> = lock(&self.machines)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        eprintln!(
            "fixture shutdown phase=stopping-machines machines={} elapsed={:?}",
            machines.len(),
            started.elapsed()
        );
        for machine in machines {
            machine.link.shutdown().await;
        }
        eprintln!(
            "fixture shutdown phase=closing-pools application={} database={} elapsed={:?}",
            self.pool.pool().size(),
            self.db.pool.size(),
            started.elapsed()
        );
        self.pool.pool().close().await;
        assert_eq!(
            self.pool.pool().size(),
            0,
            "fixture application connections are closed before database removal"
        );
        self.db.pool.close().await;
        assert_eq!(
            self.db.pool.size(),
            0,
            "fixture testkit connections are closed before database removal"
        );
        assert!(
            failures.is_empty(),
            "fixture appliers failed before shutdown: {failures:?}"
        );
        eprintln!(
            "fixture shutdown phase=quiescent owners={} elapsed={:?}",
            self.commands.live_processes(),
            started.elapsed()
        );
    }

    /// Changes the waits of the next engine.
    pub fn set_config(&self, change: impl FnOnce(&mut EngineConfig)) {
        change(&mut lock(&self.config));
    }

    /// The API restarts: the engine is built anew on the same database, the machine's link is made anew
    /// from the cursors the database holds, and the commands that did not end are handed to it.
    pub async fn restart_api(&self) {
        self.restart_api_using(|original| original).await;
    }

    /// A component proof may route a separately recorded machine through another real
    /// command supplier. The ordinary Process witness and full restart/applier lifetime stay owned.
    pub async fn restart_api_using(
        &self,
        wrap: impl FnOnce(Arc<dyn Commands>) -> Arc<dyn Commands>,
    ) {
        let before = self.hub.session_of(self.machine_id);
        self.engine().stop();
        // What is being applied right now is committed first; what comes later reaches nobody until the
        // new link stands.
        let quiet = self.gate.lock().await;
        // Nothing the process that is gone still had in hand commits from now on.
        self.appliers.fetch_add(1, Ordering::SeqCst);
        let cursors = {
            let mut conn = self.pool.acquire().await.unwrap();
            link_queries::cursors(&mut conn, self.machine_id)
                .await
                .unwrap()
        };
        let roles = vec![Role::Agent, Role::Environment, Role::Provider];
        let (link, notices) = self.hub.restart_system(self.machine_id, roles, cursors);
        // The process that is gone took its end of the connection with it, so the machine's connection
        // breaks and the machine dials again, as it does when the API it was talking to goes away.
        self.hub.faults().sever_now();
        self.commands.restart(&link);
        let engine = Engine::new(EngineParts {
            db: self.pool.clone(),
            clock: self.clock.clone(),
            config: lock(&self.config).clone(),
            commands: wrap(self.commands.current()),
            access: self.access.clone(),
            live: self.live.clone(),
        });
        engine.start();
        *lock(&self.engine) = Some(engine);
        self.spawn_applier(link, notices);
        drop(quiet);
        let (hub, machine) = (self.hub.clone(), self.machine_id);
        eventually("the machine dials again", || {
            let now = hub.session_of(machine);
            async move { now.is_some() && now != before }
        })
        .await;
    }

    /// What the API does with a machine's events: applies them in a transaction that moves the stream's
    /// cursor, and tells the link they are committed (or that they are not).
    fn spawn_applier(&self, link: ControllerLink, notices: mpsc::UnboundedReceiver<Notice>) {
        self.spawn_applier_for(self.machine_id, link, notices);
    }

    /// The same for the machine `machine`, which is the world's first or one added to it.
    fn spawn_applier_for(
        &self,
        machine: Id,
        link: ControllerLink,
        mut notices: mpsc::UnboundedReceiver<Notice>,
    ) {
        let mine = self.appliers.load(Ordering::SeqCst);
        let pool = self.pool.clone();
        let engine = self.engine();
        let clock = Arc::clone(&self.clock);
        let current = Arc::clone(&self.appliers);
        let gate = Arc::clone(&self.gate);
        let words = Arc::clone(&self.presence_words);
        let task = tokio::spawn(async move {
            while let Some(notice) = notices.recv().await {
                let gone = current.load(Ordering::SeqCst) != mine;
                match notice {
                    Notice::Deliver(delivery) => {
                        if gone {
                            link.abandoned(&delivery);
                            continue;
                        }
                        let held = gate.lock().await;
                        if current.load(Ordering::SeqCst) != mine {
                            link.abandoned(&delivery);
                            continue;
                        }
                        let applied = async {
                            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
                            if !orch_db::queries::machine_reports::accepts(
                                &mut tx,
                                machine,
                                &delivery.stream,
                                &delivery
                                    .events
                                    .iter()
                                    .map(|event| (event.kind.as_str(), &event.body))
                                    .collect::<Vec<_>>(),
                            )
                            .await
                            .map_err(|error| error.to_string())?
                            {
                                return Ok(true);
                            }
                            engine
                                .apply_events(&mut tx, machine, &delivery)
                                .await
                                .map_err(|e| e.to_string())?;
                            let first = delivery.events.first().map_or(1, |event| event.seq);
                            let moved = link_queries::advance_cursor(
                                &mut tx,
                                machine,
                                &delivery.stream.to_string(),
                                first - 1,
                                delivery.last_seq,
                                clock.now(),
                            )
                            .await
                            .map_err(|e| e.to_string())?;
                            if !moved {
                                return Ok::<bool, String>(false);
                            }
                            tx.commit().await.map_err(|e| e.to_string())?;
                            Ok(true)
                        }
                        .await;
                        match applied {
                            Ok(true) => link.committed(&delivery),
                            Ok(false) => link.abandoned(&delivery),
                            Err(error) => {
                                eprintln!("events could not be applied: {error}");
                                link.abandoned(&delivery);
                            }
                        }
                        drop(held);
                    }
                    Notice::Online { .. } if !gone => {
                        set_online(&pool, machine, true).await;
                        let _ = engine.machine_online(machine).await;
                        *lock(&words).entry(machine).or_default() += 1;
                    }
                    Notice::Offline { .. } if !gone => {
                        set_online(&pool, machine, false).await;
                        let _ = engine.machine_offline(machine).await;
                        *lock(&words).entry(machine).or_default() += 1;
                    }
                    _ => {}
                }
            }
        });
        lock(&self.applier_tasks).push(task);
    }

    /// The scripted machine answers the commands a harness answers, and keeps what it was asked.
    fn answer_commands(&self) {
        self.answer_on(&self.machine, self.machine_id, &self.seen);
    }

    /// The same for any machine of the world: what it is asked is kept in `seen` and, in the order of the
    /// whole world, in `ordered`.
    fn answer_on(&self, machine: &Arc<ScriptedMachine>, machine_id: Id, seen: &Arc<Seen>) {
        let handler = &machine.handler;
        for kind in [
            CommandKind::ProviderStart,
            CommandKind::ProviderStop,
            CommandKind::AgentStart,
            CommandKind::AgentStop,
            CommandKind::SessionOpen,
            CommandKind::SessionOpenFenced,
            CommandKind::TurnStart,
            CommandKind::TurnStartFenced,
            CommandKind::TurnSteer,
            CommandKind::TurnInterrupt,
            CommandKind::ApprovalResolve,
            CommandKind::SessionClose,
            CommandKind::SessionCloseFenced,
            CommandKind::LeaseOpen,
            CommandKind::LeaseClose,
            CommandKind::OrchToolResult,
            CommandKind::UnitAfterUp,
        ] {
            let seen = Arc::clone(seen);
            let ordered = Arc::clone(&self.ordered);
            let refusals = Arc::clone(&self.refusals);
            let declared = Arc::clone(&self.declared);
            handler.on(kind, move |command| {
                lock(&seen.commands).push(command.clone());
                lock(&ordered).push((machine_id, command.clone()));
                if let Some(error) = lock(&refusals).get_mut(&kind).and_then(VecDeque::pop_front) {
                    return Err(error);
                }
                Ok(match command {
                    Command::ProviderStart(_) => CommandResult::ProviderStart(Empty {}),
                    Command::ProviderStop(_) => CommandResult::ProviderStop(Empty {}),
                    Command::AgentStop(_) => CommandResult::AgentStop(Empty {}),
                    Command::AgentStart(_) => CommandResult::AgentStart(AgentStartResult {
                        reused: false,
                        harness_version: "1.0.0".to_owned(),
                        descriptor: serde_json::from_value(lock(&declared).clone()).unwrap(),
                    }),
                    Command::SessionOpen(open) => CommandResult::SessionOpen(SessionOpenResult {
                        native_session_ref: format!("native-{}", open.segment_id),
                        resumed: open.mode == SessionMode::Resume,
                        import_report: None,
                        instruction_sources: Vec::new(),
                        approval_reviewer: None,
                    }),
                    Command::SessionOpenFenced(open) => {
                        CommandResult::SessionOpenFenced(SessionOpenResult {
                            native_session_ref: if matches!(
                                open.session.mode,
                                SessionMode::Resume | SessionMode::Adopt
                            ) {
                                open.session.native_session_ref.clone().unwrap_or_else(|| {
                                    format!("native-{}", open.session.segment_id)
                                })
                            } else {
                                format!("native-{}", open.session.segment_id)
                            },
                            resumed: open.session.mode == SessionMode::Resume,
                            import_report: None,
                            instruction_sources: Vec::new(),
                            approval_reviewer: None,
                        })
                    }
                    Command::TurnStart(_) => CommandResult::TurnStart(Empty {}),
                    Command::TurnStartFenced(_) => CommandResult::TurnStartFenced(Empty {}),
                    Command::TurnSteer(_) => CommandResult::TurnSteer(Empty {}),
                    Command::TurnInterrupt(_) => CommandResult::TurnInterrupt(Empty {}),
                    Command::ApprovalResolve(_) => CommandResult::ApprovalResolve(Empty {}),
                    Command::SessionClose(_) => CommandResult::SessionClose(Empty {}),
                    Command::SessionCloseFenced(_) => CommandResult::SessionCloseFenced(Empty {}),
                    Command::LeaseOpen(_) => CommandResult::LeaseOpen(Empty {}),
                    Command::LeaseClose(_) => CommandResult::LeaseClose(
                        serde_json::from_value(json!({ "requests": 0, "usage": null }))
                            .map_err(|e| CommandError::new(ErrorCode::Internal, e.to_string()))?,
                    ),
                    Command::OrchToolResult(_) => CommandResult::OrchToolResult(Empty {}),
                    Command::UnitAfterUp(body) => CommandResult::UnitAfterUp(
                        orch_link_contract::commands::UnitAfterUpResult {
                            name: body.name.clone(),
                            exit: Some(0),
                            duration_ms: 0,
                            timed_out: false,
                            first_lines: String::new(),
                        },
                    ),
                    other => {
                        return Err(CommandError::new(
                            ErrorCode::Unsupported,
                            format!("no script for {}", other.kind()),
                        ));
                    }
                })
            });
        }
    }

    // ------------------------------------------------------------ more machines

    /// Adds a machine that is online, with these roles (`agent`, `environment`, `provider`), to the world.
    pub async fn add_machine(&self, name: &str, roles: &[&str]) -> Added {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = insert_machine(&mut conn, name, "connected").await.unwrap();
        let listed: Vec<String> = roles.iter().map(|role| (*role).to_owned()).collect();
        sqlx::query("UPDATE machines SET roles = $2, online = true, facts = $3, last_seen_at = clock_timestamp() WHERE id = $1")
            .bind(id)
            .bind(&listed)
            .bind(json!({
                "adapters": [{"kind":"fake","adapter_version":1,"builtin":true,"default_home_owned":true}],
                "harnesses": [{"kind":"fake","path":"/test/harness","version":"1.0.0","daemon":false}],
                "policy": {"agent.harnesses":[{"kind":"fake","path":"/test/harness"}],
                    "provider.arrangements":["separated","key_separated","credential_shared","handed_over"]}
            }))
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        let held: Vec<Role> = roles
            .iter()
            .map(|role| match *role {
                "agent" => Role::Agent,
                "environment" => Role::Environment,
                "provider" => Role::Provider,
                "node" => Role::Node,
                other => panic!("no such role {other}"),
            })
            .collect();
        let identity = self.pki.machine(id);
        let (link, notices) = self.hub.add_machine(
            id,
            held.clone(),
            Default::default(),
            MachineRecord {
                active: true,
                fingerprint: None,
            },
        );
        let machine = Arc::new(ScriptedMachine::start(
            &self.hub,
            &self.clock,
            &identity,
            &MachineSpec {
                roles: held,
                ..MachineSpec::default()
            },
        ));
        lock(&self.commands.others).insert(id, link.clone());
        let seen = Arc::new(Seen::default());
        self.answer_on(&machine, id, &seen);
        self.spawn_applier_for(id, link, notices);
        let hub = self.hub.clone();
        eventually("the added machine connects", || {
            let up = hub.session_of(id).is_some();
            async move { up }
        })
        .await;
        self.wait_for_presence_words(id, 1).await;
        lock(&self.machines).push(Arc::downgrade(&machine));
        Added { id, machine, seen }
    }

    /// Takes a machine's presence from the database: it is offline (or online) as far as the system knows.
    pub async fn set_machine_online(&self, machine: Id, online: bool) {
        set_online(&self.pool, machine, online).await;
    }

    /// Every command any machine ran, in the order they started, as the kinds they are.
    pub fn ordered_kinds(&self) -> Vec<CommandKind> {
        lock(&self.ordered)
            .iter()
            .map(|(_, command)| command.kind())
            .collect()
    }

    /// Every command any machine ran, in the order they started, with the machine.
    pub fn ordered(&self) -> Vec<(Id, Command)> {
        lock(&self.ordered).clone()
    }

    /// The tables of the database whose rows, written out as text, hold `text` or its bytes: a secret that
    /// is in the clear anywhere shows up here.
    pub async fn tables_holding(&self, text: &str) -> Vec<String> {
        let tables: Vec<String> = sqlx::query_scalar(
            "SELECT table_name::text FROM information_schema.tables
              WHERE table_schema = 'public' AND table_type = 'BASE TABLE' ORDER BY 1",
        )
        .fetch_all(self.pool.pool())
        .await
        .unwrap();
        let hex: String = text.bytes().map(|byte| format!("{byte:02x}")).collect();
        let mut found = Vec::new();
        for table in tables {
            let sql = format!(
                "SELECT count(*) FROM \"{table}\" t WHERE t::text LIKE '%' || $1 || '%' OR t::text LIKE '%' || $2 || '%'"
            );
            let hits: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
                .bind(text)
                .bind(&hex)
                .fetch_one(self.pool.pool())
                .await
                .unwrap();
            if hits > 0 {
                found.push(table);
            }
        }
        found
    }

    // ------------------------------------------------------------ the tree

    /// A thread in the topic, bound to the agent, the provider and the environment, with the model set.
    pub async fn thread(&self, name: &str) -> Id {
        self.thread_by(self.ada, name).await
    }

    pub async fn thread_by(&self, person: Id, name: &str) -> Id {
        let created = self
            .engine()
            .create_thread(
                &Caller::session(person),
                orch_engine::binding::NewThread {
                    topic: self.topic,
                    name: name.to_owned(),
                    agent: self.agent,
                    provider: self.provider,
                    environment: self.environment,
                    directory: "web".to_owned(),
                    arrangement: None,
                    model: Some(MODEL.to_owned()),
                    effort: None,
                },
            )
            .await
            .expect("a thread is created");
        created.thread
    }

    /// A person who is a member of the root org and holds nothing else.
    pub async fn person(&self, name: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        insert_user(&mut conn, id, name).await.unwrap();
        access_testing::insert_member(&mut conn, self.root, id, self.root)
            .await
            .unwrap();
        id
    }

    /// Another topic beside the first.
    pub async fn topic(&self, name: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        let id = new_id();
        insert_node(&mut conn, id, "topic", Some(self.project), name, self.ada)
            .await
            .unwrap();
        id
    }

    /// A usage cap for everyone, on a node (or anywhere), with an amount (or none, which is an exemption).
    pub async fn cap(
        &self,
        node: Option<Id>,
        amount: Option<i64>,
        enabled: bool,
        harness_kinds: &[&str],
        models: &[&str],
    ) -> Id {
        let id = new_id();
        sqlx::query(
            "INSERT INTO usage_caps (id, node_id, subject_kind, meter, period, amount, enabled, harness_kinds, models,
                                     created_by)
             VALUES ($1, $2, 'everyone', 'tokens', 'day', $3, $4, $5, $6, $7)",
        )
        .bind(id)
        .bind(node)
        .bind(amount)
        .bind(enabled)
        .bind(harness_kinds.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>())
        .bind(models.iter().map(|word| (*word).to_owned()).collect::<Vec<_>>())
        .bind(self.ada)
        .execute(self.pool.pool())
        .await
        .unwrap();
        id
    }

    /// Gives `role` on the topic to a person; the row that gives it is returned, to take it away again.
    pub async fn give(&self, person: Id, role: &str) -> Id {
        let mut conn = self.pool.acquire().await.unwrap();
        access_testing::give_role(&mut conn, self.topic, "user", person, role, self.ada, None)
            .await
            .unwrap()
    }

    /// Takes a role away.
    pub async fn take(&self, role: Id) {
        let mut conn = self.pool.acquire().await.unwrap();
        access_testing::take_role_away(&mut conn, role, self.ada, self.clock.now())
            .await
            .unwrap();
    }

    /// Writes a setting at a node, as the person who made the tree.
    pub async fn set(&self, node: Id, key: &str, value: Json) {
        let mut conn = self.pool.acquire().await.unwrap();
        settings_write::write(
            &mut conn,
            &settings_write::Write {
                actor: Actor::User(self.ada),
                target: Target::Node(node),
                key,
                value,
                locked: None,
                expected_revision: None,
                may_lock: true,
                origin: &Origin::default(),
            },
            self.clock.now(),
        )
        .await
        .unwrap();
    }

    /// Takes a setting back at a node.
    pub async fn unset(&self, node: Id, key: &str) {
        let mut conn = self.pool.acquire().await.unwrap();
        settings_write::clear(
            &mut conn,
            &settings_write::Clear {
                actor: Actor::User(self.ada),
                target: Target::Node(node),
                key,
                expected_revision: None,
                may_lock: true,
                origin: &Origin::default(),
            },
            self.clock.now(),
        )
        .await
        .unwrap();
    }

    // ------------------------------------------------------------ the database, as the tests read it

    /// Runs statements that take no parameters.
    pub async fn exec(&self, sql: &str) {
        sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
            .execute(self.pool.pool())
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }

    /// Reads one value, with one identifier bound to `$1`.
    pub async fn scalar<T>(&self, sql: &str, id: Id) -> T
    where
        T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + Send + Unpin,
    {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .bind(id)
            .fetch_one(self.pool.pool())
            .await
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
    }

    /// Counts rows, with one identifier bound to `$1`.
    pub async fn count(&self, sql: &str, id: Id) -> i64 {
        self.scalar(sql, id).await
    }

    /// Grows a thread that has at least one turn to `turns` turns by SQL: each new turn copies the first, with its items,
    /// in a few statements and with no row made one at a time. The history that results is one a person would not tell
    /// from a long one.
    pub async fn grow_history(&self, thread: Id, turns: i64) {
        let template: Id = self
            .scalar(
                "SELECT id FROM turns WHERE thread_id=$1 AND ordinal=1",
                thread,
            )
            .await;
        let first_new: i64 = self
            .scalar(
                "SELECT COALESCE(max(seq_created),0)+1000 FROM items WHERE thread_id=$1",
                thread,
            )
            .await;
        let known: i64 = self
            .scalar::<i32>("SELECT max(ordinal) FROM turns WHERE thread_id=$1", thread)
            .await
            .into();
        // One sequence slot per template item, followed by the end of its turn. The fixture also works with more
        // than ten items in its first turn, without overlapping the next turn's sequence range.
        let stride: i64 = self
            .scalar("SELECT count(*)+1 FROM items WHERE turn_id=$1", template)
            .await;
        sqlx::query(
            "WITH seed AS MATERIALIZED (SELECT to_jsonb(t) AS body FROM turns t WHERE t.id=$5)
             INSERT INTO turns SELECT copied.*
             FROM seed, generate_series($3::bigint+1,$2) g(n)
             CROSS JOIN LATERAL jsonb_populate_record(NULL::turns, seed.body || jsonb_build_object(
                'id', md5($1::text||g.n::text)::uuid, 'ordinal', g.n,
                'seq_updated', $4::bigint+g.n*$6::bigint+$6::bigint,
                'completed_seq', $4::bigint+g.n*$6::bigint+$6::bigint,
                'native_turn_ref', NULL)) AS copied",
        )
        .bind(thread)
        .bind(turns)
        .bind(known)
        .bind(first_new)
        .bind(template)
        .bind(stride)
        .execute(self.pool.pool())
        .await
        .unwrap();
        // Expanding a composite function directly as (function()).* evaluates it once for each output column.
        // A lateral record evaluates the JSON conversion once per row; the small template is materialized once.
        sqlx::query(
            "WITH seed AS MATERIALIZED (
                SELECT x.seq_created, row_number() OVER (ORDER BY x.seq_created) AS rank, to_jsonb(x) AS body
                FROM items x WHERE x.turn_id=$5)
             INSERT INTO items SELECT copied.*
             FROM seed, generate_series($3::bigint+1,$2) g(n)
             CROSS JOIN LATERAL jsonb_populate_record(NULL::items, seed.body || jsonb_build_object(
                'id', md5($1::text||g.n::text||seed.seq_created::text)::uuid,
                'turn_id', md5($1::text||g.n::text)::uuid, 'native_ref', NULL, 'search_text', NULL,
                'seq_created', $4::bigint+g.n*$6::bigint+seed.rank,
                'seq_updated', $4::bigint+g.n*$6::bigint+seed.rank)) AS copied",
        )
        .bind(thread)
        .bind(turns)
        .bind(known)
        .bind(first_new)
        .bind(template)
        .bind(stride)
        .execute(self.pool.pool())
        .await
        .unwrap();
        sqlx::query(
            "UPDATE threads SET next_seq=GREATEST(
                (SELECT COALESCE(max(seq_created),0) FROM items WHERE thread_id=$1),
                (SELECT COALESCE(max(completed_seq),0) FROM turns WHERE thread_id=$1))+1
             WHERE node_id=$1",
        )
        .bind(thread)
        .execute(self.pool.pool())
        .await
        .unwrap();
        self.exec("ANALYZE items").await;
        self.exec("ANALYZE turns").await;
    }

    /// The state of a thread: (state, blocked reason, starting step).
    pub async fn state(&self, thread: Id) -> (String, Option<String>, Option<String>) {
        sqlx::query_as(
            "SELECT state, blocked_reason, starting_step FROM threads WHERE node_id = $1",
        )
        .bind(thread)
        .fetch_one(self.pool.pool())
        .await
        .unwrap()
    }

    /// Waits for a thread to be in a state.
    pub async fn wait_state(&self, thread: Id, state: &str) {
        eventually(&format!("thread in state {state}"), || async move {
            self.state(thread).await.0 == state
        })
        .await;
    }

    /// Waits until what the end of each ended turn of a thread writes after it is written: the turn's use
    /// counted, the people and places told, its context estimated, the look at the files it changed, and the
    /// workspace's observations it overlapped made stale. That follow-up commits after the end itself, so a
    /// thread can stand idle a moment before it.
    pub async fn wait_followed_up(&self, thread: Id) {
        eventually("the follow-up of the thread's ended turns", || async move {
            self.count(
                "SELECT count(*) FROM turns
                  WHERE thread_id = $1 AND (end_work IS NOT NULL OR files_end_due OR files_work_pending)",
                thread,
            )
            .await
                == 0
        })
        .await;
    }

    /// The harness of the world's agent declares `descriptor` from now on, at every `agent.start`, and the
    /// profile says so already. Whatever starts the harness (a turn, a read of the provider's facts) keeps what
    /// it declares, so a test that changes what the adapter declares changes both. Use [`World::observed`] for
    /// initial declarations: a start already answered with an older descriptor can still save that answer.
    pub async fn declares(&self, descriptor: Json) {
        *lock(&self.declared) = descriptor.clone();
        sqlx::query("UPDATE agents SET descriptor = $2 WHERE id = $1")
            .bind(self.agent)
            .bind(descriptor)
            .execute(self.pool.pool())
            .await
            .unwrap();
    }

    /// Waits for a thread to be blocked for a reason.
    pub async fn wait_blocked(&self, thread: Id, reason: &str) {
        let started = Instant::now();
        loop {
            let (state, found, step) = self.state(thread).await;
            if state == "blocked" && found.as_deref() == Some(reason) {
                return;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "gave up waiting for: thread blocked as {reason}; it stands {state} \
                 (reason {found:?}, step {step:?})"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Waits until the people who follow a thread have been told that it is blocked for `reason`. For a
    /// block that nothing waits behind (the input it was for has failed): the next look the engine takes at
    /// a blocked thread with nothing waiting turns it idle again, which can be moments after the block was
    /// made; what the followers were told stays.
    pub async fn wait_told_blocked(&self, thread: Id, reason: &str) {
        eventually(
            &format!("the followers of the thread to be told that it is blocked as {reason}"),
            || async move {
                let told: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM notifications
                      WHERE thread_id = $1 AND kind = 'thread.blocked' AND data ->> 'reason' = $2",
                )
                .bind(thread)
                .bind(reason)
                .fetch_one(self.pool.pool())
                .await
                .unwrap();
                told > 0
            },
        )
        .await;
    }

    /// The state of a turn.
    pub async fn turn_state(&self, turn: Id) -> String {
        self.scalar("SELECT state FROM turns WHERE id = $1", turn)
            .await
    }

    /// Waits for a turn to be in a state.
    pub async fn wait_turn(&self, turn: Id, state: &str) {
        let observed = Mutex::new(None);
        let config = self.engine().config().clone();
        // Observe once per supervisor pass. The test's failure guard also
        // covers database reads, and its diagnostic shows the product's bound.
        let observing = async {
            loop {
                let found: (String, Option<Json>, Option<Json>) =
                    sqlx::query_as("SELECT state, error, usage FROM turns WHERE id = $1")
                        .bind(turn)
                        .fetch_one(self.pool.pool())
                        .await
                        .unwrap();
                let arrived = found.0 == state;
                *lock(&observed) = Some(found);
                if arrived {
                    return;
                }
                tokio::time::sleep(config.sweep_every).await;
            }
        };
        assert!(
            tokio::time::timeout(PATIENCE, observing).await.is_ok(),
            "gave up waiting for: turn {turn} in state {state} within {PATIENCE:?}; \
             observed state/error/usage {:?}; domain clock {}; configured unanswered \
             interrupt bound {:?}, supervisor interval {:?}",
            *lock(&observed),
            self.clock.now(),
            config.interrupt_force,
            config.sweep_every,
        );
    }

    /// The state of an input and what it says about it.
    /// Who answers for an input's turn when that is not its author.
    pub async fn input_run_as(&self, input: Id) -> Option<Id> {
        sqlx::query_scalar("SELECT run_as FROM thread_inputs WHERE id = $1")
            .bind(input)
            .fetch_one(self.pool.pool())
            .await
            .unwrap()
    }

    pub async fn input_state(&self, input: Id) -> (String, Option<String>) {
        sqlx::query_as("SELECT state, state_detail FROM thread_inputs WHERE id = $1")
            .bind(input)
            .fetch_one(self.pool.pool())
            .await
            .unwrap()
    }

    /// Waits for an input to be in a state.
    pub async fn wait_input(&self, input: Id, state: &str) {
        eventually(&format!("input in state {state}"), || async move {
            self.input_state(input).await.0 == state
        })
        .await;
    }

    /// The segment a thread is in now.
    pub async fn segment_of(&self, thread: Id) -> Id {
        self.scalar(
            "SELECT current_segment_id FROM threads WHERE node_id = $1",
            thread,
        )
        .await
    }

    /// What a reader is given of a thread.
    pub async fn sync(&self, thread: Id) -> orch_engine::views::ThreadSyncData {
        self.engine()
            .sync(thread, Some(self.ada), None, 500)
            .await
            .unwrap()
    }

    // ------------------------------------------------------------ the machine

    /// The `turn.start` commands the machine ran for a thread, in order.
    pub fn turn_starts(&self, thread: Id) -> Vec<TurnStart> {
        self.seen
            .all()
            .into_iter()
            .filter_map(|command| match command {
                Command::TurnStart(start) if start.thread_id == thread => Some(start),
                Command::TurnStartFenced(start) if start.thread_id == thread => Some(*start.turn),
                _ => None,
            })
            .collect()
    }

    /// Waits for the machine to have run a thread's `nth` turn start (from 0), and returns it.
    pub async fn turn_start(&self, thread: Id, nth: usize) -> TurnStart {
        let mut found = None;
        eventually(&format!("turn.start number {nth}"), || {
            found = self.turn_starts(thread).get(nth).cloned();
            let done = found.is_some();
            async move { done }
        })
        .await;
        found.expect("a turn start was seen")
    }

    /// The next commands of this kind fail with these errors, one each; the ones after them are answered as before.
    pub fn refuse(&self, kind: CommandKind, errors: Vec<CommandError>) {
        lock(&self.refusals).entry(kind).or_default().extend(errors);
    }

    /// How many times the machine began to run a command of this kind, including those it still holds.
    pub fn began(&self, kind: CommandKind) -> usize {
        self.machine
            .handler
            .executions()
            .iter()
            .filter(|run| run.kind == kind)
            .count()
    }

    /// Waits until the machine has begun `count` commands of this kind.
    pub async fn wait_began(&self, kind: CommandKind, count: usize) {
        eventually(&format!("{count} of {kind} begun"), || {
            let enough = self.began(kind) >= count;
            async move { enough }
        })
        .await;
    }

    /// Waits until a command of this kind was asked of the machine, whether or not it ran it.
    pub async fn wait_began_or_issued(&self, kind: &str) {
        eventually(&format!("a {kind} was asked for"), || {
            let asked = self.commands.issued_of(kind) > 0;
            async move { asked }
        })
        .await;
    }

    /// The machine's link goes away and does not come back until [`World::link_returns`].
    pub fn link_drops(&self) {
        self.hub.faults().refuse_connections(1_000_000);
        self.hub.faults().sever_now();
    }

    /// The machine may connect again.
    pub fn link_returns(&self) {
        self.hub.faults().refuse_connections(0);
    }

    /// The API's link says that it holds events the machine sent and has not taken them in yet, or that it
    /// holds none (`Commands::hears_behind`).
    pub fn hearing_behind(&self, behind: bool) {
        self.commands.behind.store(behind, Ordering::SeqCst);
    }

    /// How many commands of this kind the machine ran.
    pub fn ran(&self, kind: CommandKind) -> usize {
        self.seen.of_kind(kind).len()
    }

    /// Waits until the machine has run `count` commands of this kind.
    pub async fn wait_ran(&self, kind: CommandKind, count: usize) {
        eventually(&format!("{count} of {kind}"), || {
            let enough = self.ran(kind) >= count;
            async move { enough }
        })
        .await;
    }

    /// The harness says something about a thread's session.
    pub async fn say(&self, thread: Id, event: Event) {
        let segment = self.segment_of(thread).await;
        self.say_in(thread, segment, event).await;
    }

    /// The harness says something about a thread's session, in a segment.
    pub async fn say_in(&self, thread: Id, segment: Id, event: Event) {
        self.open_session_for_reports(thread, segment).await;
        self.machine
            .publish(
                &Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                &event,
            )
            .await;
    }

    /// Opens the planned harness session before this world reports from a segment.
    pub async fn open_session_for_reports(&self, thread: Id, segment: Id) {
        let Some(opening) =
            orch_testkit::doubles::sessions::prepare(self.pool.pool(), thread, segment)
                .await
                .unwrap()
        else {
            return;
        };
        let command = Outgoing::new(
            new_id(),
            opening.machine,
            orch_link_contract::commands::SerialKey::Thread(thread),
            &opening.body,
        )
        .unwrap();
        let result = orch_engine::commands::run(self.commands.current().as_ref(), command)
            .await
            .unwrap();
        let Outcome::Succeeded(result) = result else {
            panic!("the fixture session did not open: {result:?}")
        };
        orch_testkit::doubles::sessions::opened(self.pool.pool(), segment, &result)
            .await
            .unwrap();
    }

    /// The machine that keeps the provider says something of it (on the provider's stream).
    pub async fn say_of_provider(&self, event: Event) {
        self.machine
            .publish(
                &Stream::Provider {
                    provider_id: self.provider,
                },
                &event,
            )
            .await;
    }

    /// As [`World::say_of_provider`], and waits until the system has taken the event in: what it changes is
    /// in the database when this returns.
    pub async fn say_of_provider_taken_in(&self, event: Event) {
        let stream = Stream::Provider {
            provider_id: self.provider,
        };
        let name = stream.to_string();
        let before = self.taken_in(&name).await;
        self.machine.publish(&stream, &event).await;
        eventually("the event of the provider to be taken in", || async {
            self.taken_in(&name).await > before
        })
        .await;
    }

    /// Publishes a profile's diagnostic and waits until its durable report was consumed.
    pub async fn say_of_harness_taken_in(&self, event: Event) {
        let stream = Stream::Harness {
            provider_id: self.provider,
            agent_id: self.agent,
        };
        let name = stream.to_string();
        let before = self.taken_in(&name).await;
        self.machine.publish(&stream, &event).await;
        eventually("the profile report to be taken in", || async {
            self.taken_in(&name).await > before
        })
        .await;
    }

    /// How far the system has taken in the events of one stream of the world's machine.
    async fn taken_in(&self, stream: &str) -> i64 {
        let mut conn = self.pool.acquire().await.unwrap();
        link_queries::cursors(&mut conn, self.machine_id)
            .await
            .unwrap()
            .get(stream)
            .copied()
            .unwrap_or(0)
    }

    /// The arrangement a thread has.
    pub async fn arrangement_of(&self, thread: Id) -> String {
        self.scalar("SELECT arrangement FROM threads WHERE node_id = $1", thread)
            .await
    }

    /// The lease a thread has open.
    pub async fn open_lease_of(&self, thread: Id) -> Id {
        self.scalar(
            "SELECT id FROM leases WHERE thread_id = $1 AND closed_at IS NULL",
            thread,
        )
        .await
    }

    /// The rows of the usage ledger of a turn, oldest first.
    pub async fn ledger_of(&self, turn: Id) -> Vec<orch_db::usage::ledger::EventRow> {
        let mut conn = self.pool.acquire().await.unwrap();
        orch_db::usage::ledger::events_of_turn(&mut conn, turn)
            .await
            .unwrap()
    }

    /// Waits until the ledger holds `count` rows for a turn, and returns them.
    pub async fn wait_ledger(
        &self,
        turn: Id,
        count: usize,
    ) -> Vec<orch_db::usage::ledger::EventRow> {
        let started = Instant::now();
        loop {
            let rows = self.ledger_of(turn).await;
            if rows.len() >= count {
                return rows;
            }
            assert!(
                started.elapsed() < PATIENCE,
                "gave up waiting for {count} rows in the ledger, which has {}",
                rows.len()
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// The harness takes the turn up and answers with `reply`.
    pub async fn answer(&self, thread: Id, turn: &TurnStart, native_ref: &str, reply: &str) {
        self.say(thread, turn_began(turn.turn_id)).await;
        self.say(thread, message_began(turn.turn_id, native_ref))
            .await;
        self.say(thread, message_grew(native_ref, 0, reply)).await;
        self.say(thread, message_done(native_ref, reply)).await;
    }

    /// A whole turn: waits for the machine to be asked for the thread's `nth` turn, answers, and ends it.
    pub async fn run_turn(&self, thread: Id, nth: usize, reply: &str) -> TurnStart {
        let start = self.turn_start(thread, nth).await;
        self.answer(thread, &start, &format!("m{nth}"), reply).await;
        self.say(thread, turn_ended(start.turn_id, TurnOutcome::Completed))
            .await;
        self.wait_turn(start.turn_id, "completed").await;
        start
    }

    /// The thread has a turn that runs: the first input was sent, taken up, and answered in part.
    pub async fn running_turn(
        &self,
        thread: Id,
        text: &str,
    ) -> (TurnStart, orch_engine::inputs::Submitted) {
        let next_turn = self.turn_starts(thread).len();
        let submitted = self
            .engine()
            .submit_input(&Caller::session(self.ada), thread, queued(text))
            .await
            .unwrap();
        let start = self.turn_start(thread, next_turn).await;
        self.say(thread, turn_began(start.turn_id)).await;
        self.say(thread, message_began(start.turn_id, "m")).await;
        self.say(thread, message_grew("m", 0, "Working")).await;
        self.wait_turn(start.turn_id, "running").await;
        self.wait_state(thread, "running").await;
        (start, submitted)
    }
}

/// The address the tests' API provider forwards to: a port nothing listens on, since no test of the engine
/// makes a model call.
pub const UPSTREAM: &str = "http://127.0.0.1:9/v1";

/// The protocol the tests' API provider speaks.
pub const PROTOCOL: &str = "openai-responses";

/// The master key the tests seal the secrets they store with.
pub fn master_key() -> orch_db::secrets::MasterKey {
    orch_db::secrets::MasterKey::new([9; 32])
}

/// An API provider on a key the server holds, run on `machine`: its row and its key, sealed in the store.
async fn insert_api_provider(
    conn: &mut Conn,
    provider: Id,
    owner: Id,
    org: Id,
    machine: Id,
    key: &str,
    requested_at: DateTime<Utc>,
) {
    // This ready-key fixture has a recent observation attempt. Turn/binding tests must not
    // race an unrelated scheduled read; observation tests explicitly make their reads due.
    sqlx::query(
        "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, upstreams, protocols,
                                machine_id, arrangements, status, facts_requested_at)
         VALUES ($1, 'Fake key', $2, $3, $3, 'api', $4, ARRAY[$5], $6,
                 ARRAY['separated','key_separated'], 'ready', $7)",
    )
    .bind(provider)
    .bind(owner)
    .bind(org)
    .bind(json!([{ "protocol": PROTOCOL, "base_url": UPSTREAM, "options": {} }]))
    .bind(PROTOCOL)
    .bind(machine)
    .bind(requested_at)
    .execute(&mut *conn)
    .await
    .unwrap();
    put_secret(conn, provider, "api_key", key).await;
}

/// Stores a secret of a provider, sealed as the routes seal it.
pub async fn put_secret(conn: &mut Conn, provider: Id, name: &str, value: &str) -> Id {
    let sealed = master_key()
        .seal(
            value.as_bytes(),
            format!("secrets/provider/{provider}/{name}").as_bytes(),
        )
        .unwrap();
    let stored = orch_db::queries::settings::put_secret(
        conn,
        "provider",
        provider,
        name,
        &sealed,
        None,
        Utc::now(),
    )
    .await
    .unwrap();
    stored.id
}

async fn set_online(pool: &Db, machine: Id, online: bool) {
    let mut conn = pool.acquire().await.unwrap();
    orch_db::queries::resources::set_presence(&mut conn, machine, online, None, Utc::now())
        .await
        .unwrap();
}

fn build_engine(
    pool: &Db,
    clock: &Arc<dyn Clock>,
    config: &EngineConfig,
    commands: &Arc<ScriptedCommands>,
    access: &Arc<TestAccess>,
    live: &Arc<Recorder>,
) -> Engine {
    Engine::new(EngineParts {
        db: pool.clone(),
        clock: Arc::clone(clock),
        config: config.clone(),
        commands: commands.current(),
        access: access.clone(),
        live: live.clone(),
    })
}
