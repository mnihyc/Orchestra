//! Provider/profile homes, the quiet and pinned start, a harness held to the MCP servers of its session, its own memory pointed at the thread's directory or switched off, supervision of one process or one for each conversation, and session recovery.
//!
//! A **harness process** is what the connector hosts for one provider and one agent profile: one
//! adapter, started once, serving every session of every thread bound to that pair, each in its own
//! session. [`Hosting`] is all of them; [`Process`] is one.
//!
//! What the connector does for a process, and what it leaves to the adapter:
//!
//! * It **registers** adapters by kind ([`AdapterRegistry`]); a kind with no registration is
//!   `harness_unavailable`. The built-in adapters and plugins are registered the same way.
//! * It gives each process a **home** of its own: the one the system named (relative to the data
//!   directory, for a managed runtime), or the profile's, which on a person's machine has to be inside
//!   a directory its owner offers, or `harness/<provider>/<agent>` under the data directory.
//! * It finds the **program** only where the machine's owner listed it (`agent.harnesses`); a profile
//!   may name the program but cannot name another than the owner's.
//! * It gives the process a **token** for the connector's listener, and each session a key under it.
//! * It reads the process's **events** and puts them in the spool in order, text bundled; it tracks the
//!   sessions and the turn that runs in each, from the events, and keeps that on disk ([`Ledger`]).
//! * It **supervises**: a process that dies (the adapter says so, or stops reporting) is reported, its
//!   sessions are reported lost, and it is started again after a pause that doubles from a second, at
//!   most `[adapter] harness_restarts` times within `harness_restart_window_secs`, after which it is
//!   left failed until the system starts it again.
//!
//! The quiet and pinned start (nothing that is not the work leaves the machine, the harness does not
//! update itself), holding a session to the tool servers it is given and pointing a harness's memory at
//! the thread's directory are the adapter's: it is given the profile's `config` and `env` to merge over
//! its own base configuration, the session's `mcp` and `memory`, and starts its harness accordingly.

pub(crate) mod output_blobs;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart as AdapterStart, EventSink, EventStream,
};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::events::SessionEvent;
use orch_agent_contract::facts::{AgentProfile, HarnessState, HarnessStatus};
use orch_agent_contract::loopback::Loopback as AdapterLoopback;
use orch_agent_contract::session::ThreadId;
use orch_link_contract::commands::{
    AgentStart, AgentStartResult, CommandError, ErrorCode, SandboxMode, Secret as LinkSecret,
    SecretOrText,
};
use orch_link_contract::events::{Event, RawFrame, SessionSandbox};
use orch_link_contract::session::MachineFactsAdapter;
use orch_link_contract::streams::Stream;
use orch_types::{Clock, Field, Id, Json};

use crate::events::{coalesce, convert, harness_status};
use crate::facts::FactsDraft;
use crate::ledger::{HostedHarness, HostedSession, Ledger};
use crate::loopback::{Grant, SessionBinding, Tickets};
use crate::policy::Clamp;
use crate::roles::Services;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct RuntimeEnvironment {
    fingerprint: orch_types::Hash,
    variables: BTreeMap<String, String>,
}

fn runtime_environment_name(provider: Id, agent: Id) -> String {
    format!("harness-runtime-{provider}-{agent}")
}

/// Native sign-ins live with the environment home, separately from replaceable connector identity.
fn environment_connection_home(root: &Path, provider: Id, agent: Id) -> PathBuf {
    root.join(".orch")
        .join("provider-connections")
        .join(provider.to_string())
        .join(agent.to_string())
}

fn process_environment(services: &Services, mut start: AdapterStart) -> AdapterStart {
    if services.policy.policy().is_none() {
        // A managed unit has one environment and needs no peer token. Refresh the local URL
        // on every native restart; the persisted requested profile is never rewritten.
        start.profile.env.remove("ORCH_PORTS_URL");
        start.profile.env.remove("ORCH_PORTS_TOKEN");
        if let Some(url) = services.loopback.environment_ports_url() {
            start.profile.env.insert("ORCH_PORTS_URL".into(), url);
        }
    }
    start
}

/// The first pause before a harness that died is started again: a second, as the link's own first
/// pause. Each further restart within the window waits twice as long as the one before.
pub const RESTART_FIRST_PAUSE: Duration = Duration::from_secs(1);
/// The longest pause before a restart: the link's longest pause.
pub const RESTART_LONGEST_PAUSE: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------------------------------------
// The registry

/// Where a harness process gets its home when nothing names one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeRule {
    /// A directory of its own under the data directory: `harness/<provider id>/<agent id>`.
    Connector,
    /// None is given: the harness has its own default home, and the adapter uses it.
    Harness,
}

/// Makes the adapter of a kind.
pub type AdapterFactory = dyn Fn(Arc<dyn Clock>) -> Box<dyn AgentAdapter> + Send + Sync;

/// One adapter the connector can host.
#[derive(Clone)]
pub struct AdapterRegistration {
    /// The harness kind it drives.
    pub kind: &'static str,
    /// The version of its declarations.
    pub adapter_version: u32,
    /// Whether it is part of the connector and no program beside it.
    pub builtin: bool,
    /// Whether it needs the program of a harness: its location is then the owner's to list, on a
    /// person's machine.
    pub needs_binary: bool,
    /// Where its processes get their homes.
    pub home_rule: HomeRule,
    /// Makes one.
    pub factory: Arc<AdapterFactory>,
}

impl std::fmt::Debug for AdapterRegistration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdapterRegistration")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// Two adapters of one kind.
#[derive(Debug, thiserror::Error)]
#[error("an adapter for the harness kind {0:?} is registered already")]
pub struct DuplicateAdapter(pub &'static str);

/// The adapters a connector can host, by the kind of harness each drives.
#[derive(Debug, Clone, Default)]
pub struct AdapterRegistry {
    adapters: Arc<Mutex<BTreeMap<&'static str, AdapterRegistration>>>,
    /// The names a harness's program answers to, by harness kind, for the kinds whose program is not named
    /// as the kind is.
    programs: BTreeMap<&'static str, Vec<&'static str>>,
    /// The manifests of the programs whose adapters are not part of the connector, by kind: they are told to
    /// the system with the machine's facts, so that the program is listed and its builds are checked like any
    /// other's.
    manifests: BTreeMap<&'static str, orch_agent_programs::ProgramManifest>,
}

impl AdapterRegistry {
    /// A registry with no adapter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an adapter; one kind has one.
    pub fn register(&mut self, registration: AdapterRegistration) -> Result<(), DuplicateAdapter> {
        let mut adapters = lock(&self.adapters);
        if adapters.contains_key(registration.kind) {
            return Err(DuplicateAdapter(registration.kind));
        }
        adapters.insert(registration.kind, registration);
        Ok(())
    }

    /// Says which names the program of the harness of `kind` answers to, where the program is not named as
    /// the kind is (the kind `claude-code` runs the program `claude`).
    pub fn program_names(&mut self, kind: &'static str, names: &[&str]) {
        self.programs.insert(
            kind,
            names
                .iter()
                .map(|name| orch_agent_programs::intern(name))
                .collect(),
        );
    }

    /// Keeps the manifest of the program of `kind`, to be told to the system.
    pub fn set_manifest(
        &mut self,
        kind: &'static str,
        manifest: orch_agent_programs::ProgramManifest,
    ) {
        self.manifests.insert(kind, manifest);
    }

    /// The adapter of a kind.
    #[must_use]
    pub fn get(&self, kind: &str) -> Option<AdapterRegistration> {
        lock(&self.adapters).get(kind).cloned()
    }

    /// The one external-program supervisor atomically replaces its factories; built-ins remain reserved.
    pub fn replace_external(
        &self,
        registrations: Vec<AdapterRegistration>,
    ) -> Result<(), DuplicateAdapter> {
        let mut adapters = lock(&self.adapters);
        let mut next = adapters.clone();
        next.retain(|_, entry| entry.builtin);
        for entry in registrations {
            if next.contains_key(entry.kind) {
                return Err(DuplicateAdapter(entry.kind));
            }
            next.insert(entry.kind, entry);
        }
        *adapters = next;
        Ok(())
    }

    /// Validate a complete external-registry replacement before its configuration is persisted.
    pub fn check_external(
        &self,
        registrations: &[AdapterRegistration],
    ) -> Result<(), DuplicateAdapter> {
        let adapters = lock(&self.adapters);
        let mut seen = adapters
            .iter()
            .filter(|(_, entry)| entry.builtin)
            .map(|(kind, _)| *kind)
            .collect::<std::collections::BTreeSet<_>>();
        for entry in registrations {
            if !seen.insert(entry.kind) {
                return Err(DuplicateAdapter(entry.kind));
            }
        }
        Ok(())
    }

    /// The names the program of the harness of `kind` answers to: those it was given, or the kind itself.
    #[must_use]
    pub fn programs_of<'a>(&'a self, kind: &'a str) -> Vec<&'a str> {
        self.programs
            .get(kind)
            .map_or_else(|| vec![kind], |names| names.clone())
    }

    /// The kinds of harness whose adapter needs the program of a harness, in the order of their names.
    #[must_use]
    pub fn kinds_with_a_program(&self) -> Vec<&'static str> {
        lock(&self.adapters)
            .values()
            .filter(|registration| registration.needs_binary)
            .map(|registration| registration.kind)
            .collect()
    }

    /// The adapters, as the machine's facts list them.
    #[must_use]
    pub fn facts(&self) -> Vec<MachineFactsAdapter> {
        lock(&self.adapters)
            .values()
            .map(|registration| MachineFactsAdapter {
                kind: registration.kind.to_owned(),
                adapter_version: registration.adapter_version,
                builtin: registration.builtin,
                default_home_owned: registration.home_rule == HomeRule::Connector,
                needs_binary: Some(registration.needs_binary),
                manifest: self.manifests.get(registration.kind).cloned(),
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------------------------------------
// Errors

/// What an adapter's error is as a command's.
#[must_use]
pub fn command_error(error: &AdapterError) -> CommandError {
    CommandError {
        code: error.code().to_owned(),
        detail: error.detail(),
        retryable: error.retryable(),
        details: None,
    }
}

fn unavailable(detail: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::HarnessUnavailable, detail)
}

fn unavailable_by_policy(key: orch_link_contract::policy::PolicyKey, detail: &str) -> CommandError {
    let mut error = unavailable(format!("{key}: {detail}"));
    error.details = Some(serde_json::json!({"policy_key":key.as_str()}));
    error
}

type Key = (Id, Id);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------------------------
// A process

struct OpenSession {
    opening_id: Id,
    opening_epoch: i64,
    native_environment: Option<Id>,
    directory: String,
    cwd: String,
    segment_id: Id,
    native_session_ref: String,
    continuation_fenced: bool,
    session_key: String,
    running_turn: Option<Id>,
    background: std::collections::HashSet<String>,
    awaiting_native: bool,
}

impl OpenSession {
    fn configuration_directories(
        &self,
        environment: Id,
        root: &str,
    ) -> std::collections::BTreeSet<String> {
        if self.native_environment != Some(environment) {
            return std::collections::BTreeSet::new();
        }
        let mut found =
            std::collections::BTreeSet::from([self.directory.clone(), self.cwd.clone()]);
        for directory in Path::new(&self.cwd)
            .ancestors()
            .take_while(|directory| directory.starts_with(root))
        {
            found.insert(directory.to_string_lossy().into_owned());
        }
        found
    }

    fn operation(&self, expected: Option<Id>) -> Result<(), CommandError> {
        if expected.is_some_and(|id| id != self.segment_id)
            || expected.is_none() && self.continuation_fenced
        {
            let mut error = CommandError::new(
                ErrorCode::Busy,
                "The thread's native session changed; this operation names another continuation.",
            );
            error.code = "session.segment_changed".into();
            return Err(error);
        }
        Ok(())
    }

    fn refresh(&self, segment: Id, native: Option<&str>) -> Result<(), CommandError> {
        if self.continuation_fenced
            && (self.segment_id != segment || native != Some(self.native_session_ref.as_str()))
        {
            let mut error = CommandError::new(
                ErrorCode::Busy,
                "This session setup names an older native lane; the current continuation is kept.",
            );
            error.code = "session.segment_changed".into();
            return Err(error);
        }
        Ok(())
    }

    fn replacement(
        &self,
        source: &orch_agent_contract::history::NativePrefixSource,
        replace: Option<Id>,
        retained: Option<&orch_agent_contract::history::NativePrefixSource>,
    ) -> Result<Option<Id>, CommandError> {
        let retained = retained.is_some_and(|retained| {
            retained.thread_id == source.thread_id
                && retained.segment_id == self.segment_id
                && retained.native_session_ref == self.native_session_ref
        });
        if self.running_turn.is_some()
            || !self.background.is_empty()
            || self.awaiting_native
            || !(self.segment_id == source.segment_id
                && self.native_session_ref == source.native_session_ref
                || replace == Some(self.segment_id)
                || retained)
        {
            return Err(CommandError::new(
                ErrorCode::Busy,
                "Another or still-running native session occupies this thread.",
            ));
        }
        Ok(if retained {
            Some(self.segment_id)
        } else {
            replace
        })
    }
}

struct State {
    status: HarnessState,
    detail: Option<String>,
    version: String,
    descriptor: Option<AgentDescriptor>,
    sessions: HashMap<ThreadId, OpenSession>,
    restarts: VecDeque<tokio::time::Instant>,
    /// What the sign-in under way last asked the person for, as the adapter reported it.
    login_asks: Vec<Field>,
}

/// What every process of the connector shares.
struct Context {
    services: Arc<Services>,
    ledger: Arc<Ledger>,
    threads: Mutex<HashMap<ThreadId, (Key, Id)>>,
    mcp_changed: Arc<tokio::sync::Notify>,
    restarts: u32,
    window: Duration,
}

/// Why the process of a harness is dead, when what the adapter reported says so: the harness failed, or its
/// process was lost and the adapter says why. An adapter that ended its process on purpose, to start it again
/// with a sign-in it was given, reports it offline and has nothing to say; that is not a death, and what the
/// adapter reports after it (the sign-in that completed) must still be heard.
fn death_of(status: &HarnessStatus) -> Option<String> {
    match status.status {
        HarnessState::Error => Some(
            status
                .detail
                .clone()
                .unwrap_or_else(|| "the harness process is gone".to_owned()),
        ),
        HarnessState::Offline => status.detail.clone(),
        _ => None,
    }
}

type Scratch = HashMap<(Option<Id>, Id), Vec<Id>>;

/// One harness process: an adapter, its home, its sessions.
pub struct Process {
    /// The provider it serves.
    pub provider_id: Id,
    /// The agent profile it runs.
    pub agent_id: Id,
    /// The harness kind.
    pub kind: String,
    ctx: Arc<Context>,
    request: Mutex<AgentStart>,
    adapter: tokio::sync::Mutex<Box<dyn AgentAdapter>>,
    interrupt: Mutex<Option<orch_agent_contract::adapter::Interrupt>>,
    output_directory: Mutex<Option<PathBuf>>,
    adapter_start: AdapterStart,
    resource_group: Option<orch_envhost::process_limits::UserGroup>,
    owned_home: bool,
    grant: Grant,
    state: Mutex<State>,
    stopping: AtomicBool,
    generation: AtomicU64,
    /// The commands whose files a turn is still using, by the turn.
    scratch: Mutex<Scratch>,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process")
            .field("provider", &self.provider_id)
            .field("agent", &self.agent_id)
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Process {
    /// Commands after attach use the very same aggregate as this provider's harnesses.
    pub(crate) fn resource_launcher(&self) -> Vec<String> {
        self.resource_group
            .as_ref()
            .map_or_else(Vec::new, orch_envhost::process_limits::UserGroup::launcher)
    }
    /// The actual selected harness home and resolved variables for the shared after-start runner.
    pub(crate) fn after_up_context(
        &self,
    ) -> Result<(PathBuf, BTreeMap<String, String>), CommandError> {
        let home = self
            .adapter_start
            .home
            .as_ref()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from)
            })
            .ok_or_else(|| unavailable("this runtime has not reported a usable home directory"))?;
        Ok((home, self.adapter_start.profile.env.clone()))
    }
    fn key(&self) -> Key {
        (self.provider_id, self.agent_id)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.state)
    }

    /// The version of the harness that runs and what its adapter declares of it.
    #[must_use]
    pub fn started(&self) -> (String, Option<AgentDescriptor>) {
        let state = self.state();
        (state.version.clone(), state.descriptor.clone())
    }

    /// The fields of one of the adapter's sign-in methods, as it declared them; none for a method it did
    /// not declare. They say what type each value of the sign-in has, which a command carries as text.
    #[must_use]
    pub fn login_fields(&self, method: &str) -> Vec<Field> {
        self.state()
            .descriptor
            .as_ref()
            .and_then(|descriptor| descriptor.native_provider.as_ref())
            .and_then(|native| native.login_methods.iter().find(|each| each.id == method))
            .map(|method| method.fields.clone())
            .unwrap_or_default()
    }

    /// What the sign-in under way last asked the person for, as the adapter reported it.
    #[must_use]
    pub fn login_asks(&self) -> Vec<Field> {
        self.state().login_asks.clone()
    }

    /// The request this process was started with.
    #[must_use]
    pub fn request(&self) -> AgentStart {
        lock(&self.request).clone()
    }

    /// The token of the process for the connector's listener.
    #[must_use]
    pub fn grant(&self) -> &Grant {
        &self.grant
    }

    /// Keeps the files that `command` fetched until the turn `turn` ends: the harness reads them while
    /// it works.
    pub fn keep_for_turn(&self, opening_id: Option<Id>, turn: Id, command: Id) {
        lock(&self.scratch)
            .entry((opening_id, turn))
            .or_default()
            .push(command);
    }

    /// How many sessions are open in it.
    #[must_use]
    pub fn sessions_open(&self) -> usize {
        self.state().sessions.len()
    }

    /// The turn that runs in the session of a thread, if the session is open and one does.
    #[must_use]
    pub fn running_turn(&self, thread: ThreadId) -> Option<Id> {
        self.state()
            .sessions
            .get(&thread)
            .and_then(|session| session.running_turn)
    }

    /// Read the listener binding and reserved turn in one state cut. An old URL cannot
    /// borrow a replacement segment/turn from the process's thread-wide lookup.
    pub fn orch_tool_source(
        &self,
        binding: &SessionBinding,
    ) -> Option<crate::ledger::ToolCallSource> {
        if binding.grant != self.grant.id
            || binding.provider_id != self.provider_id
            || binding.agent_id != self.agent_id
        {
            return None;
        }
        let state = self.state();
        let session = state.sessions.get(&binding.thread_id)?;
        if session.segment_id != binding.segment_id
            || session.session_key != binding.session_key
            || session.native_session_ref.is_empty()
        {
            return None;
        }
        Some(crate::ledger::ToolCallSource {
            segment_id: session.segment_id,
            turn_id: session.running_turn?,
            native: orch_types::Hash::of(session.native_session_ref.as_bytes()),
        })
    }

    /// The key of a thread's session, if it is open here.
    #[must_use]
    pub fn session_key(&self, thread: ThreadId) -> Option<String> {
        self.state()
            .sessions
            .get(&thread)
            .map(|session| session.session_key.clone())
    }

    /// Native work keeps its exact slot while configuration refresh is deferred.
    pub fn background_running(&self, thread: ThreadId) -> bool {
        self.state()
            .sessions
            .get(&thread)
            .is_some_and(|session| !session.background.is_empty() || session.awaiting_native)
    }

    /// The actual native identity of this currently installed session slot.
    pub fn native_session_ref(&self, thread: ThreadId) -> Option<String> {
        self.state()
            .sessions
            .get(&thread)
            .map(|session| session.native_session_ref.clone())
    }

    /// Runs one call of the actual process, without borrowing its replacement after waiting for the adapter.
    pub async fn call<R>(
        &self,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        self.call_at(self.generation(), work).await
    }

    fn serves_generation(&self, generation: u64) -> bool {
        self.generation() == generation
            && !self.stopping.load(Ordering::SeqCst)
            && self.state().status == HarnessState::Online
    }

    /// Executes only on the captured process generation, including after the adapter's queue wait.
    pub async fn call_at<R>(
        &self,
        generation: u64,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        if !self.serves_generation(generation) {
            return Err(unavailable(
                "the native process changed before this call could begin",
            ));
        }
        let mut adapter = self.adapter.lock().await;
        if !self.serves_generation(generation) {
            return Err(unavailable(
                "the native process changed while this call was waiting",
            ));
        }
        let result = work(adapter.as_mut()).await;
        *lock(&self.interrupt) = adapter.interrupt_handle();
        result.map_err(|error| command_error(&error))
    }

    /// Stops one exact turn without joining an adapter call that may be exporting or replaying history.
    pub async fn interrupt(
        &self,
        context: &orch_link::commands::CommandContext,
        thread: ThreadId,
        turn: Id,
    ) -> Result<(), CommandError> {
        let generation = self.generation();
        let handle = lock(&self.interrupt).clone();
        if let Some(handle) = handle {
            self.check_call_target(context, generation, None).await?;
            return handle(thread, turn)
                .await
                .map_err(|error| command_error(&error));
        }
        self.call_target(context, |adapter| adapter.interrupt(thread, turn))
            .await
    }

    async fn publish(&self, stream: Stream, event: Event) {
        self.publish_from(None, stream, event).await;
    }

    async fn publish_from(&self, native_opening_id: Option<Id>, stream: Stream, event: Event) {
        if let Err(error) = self
            .ctx
            .services
            .publisher
            .publish_from(native_opening_id, stream, event)
            .await
        {
            tracing::warn!(%error, "an event could not be put in the spool");
        }
    }

    fn harness_stream(&self) -> Stream {
        Stream::Harness {
            provider_id: self.provider_id,
            agent_id: self.agent_id,
        }
    }

    async fn publish_status(&self, status: HarnessState, detail: Option<String>) {
        let (version, sessions) = {
            let state = self.state();
            (state.version.clone(), state.sessions.len())
        };
        let pin = lock(&self.request).profile.harness_version.clone();
        let detail = if status == HarnessState::Online {
            orch_agent_contract::facts::build_detail(pin.as_deref(), &version, detail)
        } else {
            detail
        };
        self.state().detail = detail.clone();
        let report = HarnessStatus {
            home: Some(self.home_fact()),
            status,
            detail,
            harness_version: Some(version),
            sessions_open: u32::try_from(sessions).unwrap_or(u32::MAX),
        };
        self.publish(
            self.harness_stream(),
            Event::HarnessStatus(harness_status(&report)),
        )
        .await;
    }

    async fn publish_descriptor(&self) {
        let descriptor = self.state().descriptor.clone();
        if let Some(descriptor) = descriptor {
            self.publish(self.harness_stream(), Event::HarnessDescriptor(descriptor))
                .await;
        }
    }

    // ---- sessions

    /// Refuses an operation for a replaced or closed native session generation.
    pub fn has_segment(&self, thread: ThreadId, segment: Id) -> bool {
        self.state()
            .sessions
            .get(&thread)
            .is_some_and(|session| session.segment_id == segment)
    }

    /// A retained lane may be observed only while idle, never over a later generation.
    /// No open slot is allowed: the adapter then verifies the retained native source itself.
    pub fn check_idle_source(
        &self,
        source: &orch_agent_contract::history::NativePrefixSource,
        replace_segment: Option<Id>,
    ) -> Result<(), CommandError> {
        self.idle_resume_replacement(source, replace_segment, None)
            .map(|_| ())
    }

    /// The auxiliary source is matched by its full native identity, never just a reused thread key.
    pub fn idle_resume_replacement(
        &self,
        source: &orch_agent_contract::history::NativePrefixSource,
        replace_segment: Option<Id>,
        retained: Option<&orch_agent_contract::history::NativePrefixSource>,
    ) -> Result<Option<Id>, CommandError> {
        let state = self.state();
        match state.sessions.get(&source.thread_id) {
            Some(session) => session.replacement(source, replace_segment, retained),
            None => Ok(replace_segment),
        }
    }

    /// A continuation lane accepts only operations naming its actual segment.
    pub fn check_segment(
        &self,
        thread: ThreadId,
        expected: Option<Id>,
    ) -> Result<(), CommandError> {
        let state = self.state();
        let session = state
            .sessions
            .get(&thread)
            .ok_or_else(|| unavailable("The thread's native session is not open."))?;
        session.operation(expected)
    }

    /// Refreshing configuration is not permission to replace another prepared native lane.
    pub fn check_refresh(
        &self,
        thread: ThreadId,
        segment: Id,
        native: Option<&str>,
    ) -> Result<(), CommandError> {
        let state = self.state();
        state
            .sessions
            .get(&thread)
            .map_or(Ok(()), |session| session.refresh(segment, native))
    }

    #[must_use]
    pub fn continuation_fenced(&self, thread: ThreadId) -> bool {
        self.state()
            .sessions
            .get(&thread)
            .is_some_and(|session| session.continuation_fenced)
    }

    /// Called only after a successful exact preparation/resume has installed its slot.
    pub fn fence_continuation(&self, thread: ThreadId) {
        if let Some(session) = self.state().sessions.get_mut(&thread) {
            session.continuation_fenced = true;
        }
    }

    /// The actual process generation a native call is about to use.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// A session opened in the adapter: it is kept, in the process and on disk.
    pub fn session_opened(
        &self,
        session_key: &str,
        session: HostedSession,
        opening_epoch: i64,
        generation: u64,
        native_environment: Option<Id>,
    ) -> Result<(), CommandError> {
        let mut state = self.state();
        if generation != self.generation()
            || self.stopping.load(Ordering::SeqCst)
            || state.status != HarnessState::Online
        {
            return Err(unavailable(
                "the process that opened this session has already ended",
            ));
        }
        state.sessions.insert(
            session.thread_id,
            OpenSession {
                opening_id: session.opening_id,
                opening_epoch,
                native_environment,
                directory: session.directory.clone(),
                cwd: session.cwd.clone(),
                segment_id: session.segment_id,
                native_session_ref: session.native_session_ref.clone(),
                continuation_fenced: false,
                session_key: session_key.to_owned(),
                running_turn: session.running_turn,
                background: Default::default(),
                awaiting_native: false,
            },
        );
        if native_environment.is_some() {
            self.ctx.mcp_changed.notify_one();
        }
        lock(&self.ctx.threads).insert(session.thread_id, (self.key(), session.opening_id));
        self.ctx
            .ledger
            .session_opened(self.provider_id, self.agent_id, session);
        Ok(())
    }

    /// The exact physical slot, captured before an operation waits elsewhere.
    #[must_use]
    pub fn session_target(
        &self,
        thread: ThreadId,
    ) -> Option<orch_link_contract::commands::SessionTarget> {
        self.state().sessions.get(&thread).map(|session| {
            orch_link_contract::commands::SessionTarget {
                thread_id: thread,
                segment_id: Some(session.segment_id),
                opening_id: Some(session.opening_id),
                opening_epoch: Some(session.opening_epoch),
            }
        })
    }

    /// Rechecks a physical pin after the adapter queue; an absent or superseded close is a no-op.
    pub async fn call_session<R>(
        &self,
        target: &orch_link_contract::commands::SessionTarget,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<Option<R>, CommandError> {
        if target.opening_id.is_none() {
            return Ok(None);
        }
        let generation = self.generation();
        let mut adapter = self.adapter.lock().await;
        if !self.serves_generation(generation)
            || self.session_target(target.thread_id).as_ref() != Some(target)
        {
            return Ok(None);
        }
        work(adapter.as_mut())
            .await
            .map(Some)
            .map_err(|error| command_error(&error))
    }

    /// Applies a session operation only to its accepted physical slot after the adapter queue.
    pub async fn call_target<R>(
        &self,
        context: &orch_link::commands::CommandContext,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        self.call_target_checked(context, None, work).await
    }

    pub async fn call_config<R>(
        &self,
        context: &orch_link::commands::CommandContext,
        action: orch_link_contract::native_target::SessionAction,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        self.call_target_checked(context, Some(action), work).await
    }

    async fn call_target_checked<R>(
        &self,
        context: &orch_link::commands::CommandContext,
        action: Option<orch_link_contract::native_target::SessionAction>,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        let generation = self.generation();
        let mut adapter = self.adapter.lock().await;
        self.check_call_target(context, generation, action).await?;
        work(adapter.as_mut())
            .await
            .map_err(|error| command_error(&error))
    }

    async fn check_call_target(
        &self,
        context: &orch_link::commands::CommandContext,
        generation: u64,
        action: Option<orch_link_contract::native_target::SessionAction>,
    ) -> Result<(), CommandError> {
        let Some(orch_link_contract::commands::NativeTarget::Session(target)) =
            context.native_target.as_ref()
        else {
            return Err(unavailable("the command has no captured native session"));
        };
        if !self.serves_generation(generation)
            || self.session_target(target.thread_id).as_ref() != Some(target)
        {
            return Err(CommandError::new(
                ErrorCode::SessionNotFound,
                "the captured physical session ended while this call was waiting",
            ));
        }
        let fences = context
            .native_fences
            .clone()
            .ok_or_else(|| unavailable("the native ordering store is unavailable"))?;
        let target = target.clone();
        let captured = target.clone();
        let live = tokio::task::spawn_blocking(move || {
            if !fences.tool_control().1 {
                return Ok(false);
            }
            fences.observe_session(
                &target,
                orch_link_contract::native_target::SessionAction::Use,
            )
        })
        .await
        .map_err(|_| unavailable("the native ordering check failed"))?
        .map_err(|_| unavailable("the native ordering marker cannot be read"))?;
        if !live
            || !self.serves_generation(generation)
            || self.session_target(captured.thread_id).as_ref() != Some(&captured)
            || context.cancel.is_cancelled()
        {
            return Err(CommandError::new(
                ErrorCode::SessionNotFound,
                "the captured physical session was retired while this call was waiting",
            ));
        }
        if let Some(action) = action {
            let current = self.session_key(captured.thread_id).is_some_and(|key| {
                context.native_members.as_ref().is_some_and(|members| {
                    self.ctx
                        .services
                        .loopback
                        .configuration_current(&key, action, members)
                })
            });
            if !current {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the accepted session configuration was superseded",
                ));
            }
        }
        Ok(())
    }

    /// Checks an opening's persistent order after queued preparation and before native creation.
    pub async fn call_opening<R>(
        &self,
        context: &orch_link::commands::CommandContext,
        generation: u64,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        let mut adapter = self.adapter.lock().await;
        if !self.serves_generation(generation) {
            return Err(unavailable(
                "the native process changed while this opening was waiting",
            ));
        }
        let Some(orch_link_contract::commands::NativeTarget::Session(target)) =
            context.native_target.as_ref()
        else {
            return Err(unavailable("the command has no captured native opening"));
        };
        let fences = context
            .native_fences
            .clone()
            .ok_or_else(|| unavailable("the native ordering store is unavailable"))?;
        let target = target.clone();
        let live = tokio::task::spawn_blocking(move || {
            if !fences.tool_control().1 {
                return Ok(false);
            }
            fences.observe_session(
                &target,
                orch_link_contract::native_target::SessionAction::Open,
            )
        })
        .await
        .map_err(|_| unavailable("the native ordering check failed"))?
        .map_err(|_| unavailable("the native ordering marker cannot be read"))?;
        if !live || !self.serves_generation(generation) || context.cancel.is_cancelled() {
            return Err(CommandError::new(
                ErrorCode::SessionNotFound,
                "the captured physical opening was retired while waiting",
            ));
        }
        work(adapter.as_mut())
            .await
            .map_err(|error| command_error(&error))
    }

    /// A session ended: everything kept of it goes.
    pub fn session_closed(&self, thread: ThreadId) {
        let opening = self
            .state()
            .sessions
            .get(&thread)
            .map(|session| session.opening_id);
        if let Some(opening) = opening {
            self.session_closed_if(thread, opening);
        }
    }

    /// Removes only this physical opening, keeping a replacement's slot, ledger and local key intact.
    pub fn session_closed_if(&self, thread: ThreadId, opening_id: Id) {
        let mut state = self.state();
        if !state
            .sessions
            .get(&thread)
            .is_some_and(|session| session.opening_id == opening_id)
        {
            return;
        }
        let Some(session) = state.sessions.remove(&thread) else {
            return;
        };
        let mut threads = lock(&self.ctx.threads);
        if threads.get(&thread) == Some(&(self.key(), opening_id)) {
            threads.remove(&thread);
        }
        self.ctx.ledger.session_closed_if(thread, opening_id);
        if session.native_environment.is_some() {
            self.ctx.mcp_changed.notify_one();
        }
        self.ctx.services.loopback.unbind(&session.session_key);
    }

    /// Removes only the matching native history's local slot, preserving a successor or another home.
    pub fn history_removed(&self, source: &orch_agent_contract::history::NativePrefixSource) {
        let mut state = self.state();
        if !state
            .sessions
            .get(&source.thread_id)
            .is_some_and(|session| {
                session.native_session_ref == source.native_session_ref
                    && session.running_turn.is_none()
            })
        {
            return;
        }
        let Some(session) = state.sessions.remove(&source.thread_id) else {
            return;
        };
        let mut threads = lock(&self.ctx.threads);
        if threads.get(&source.thread_id) == Some(&(self.key(), session.opening_id)) {
            threads.remove(&source.thread_id);
            self.ctx
                .ledger
                .session_closed_if(source.thread_id, session.opening_id);
        }
        drop(threads);
        drop(state);
        self.ctx.services.loopback.unbind(&session.session_key);
    }

    /// Reserves a just-admitted turn before the adapter can return ahead of its TurnStarted event.
    pub fn reserve_turn(&self, thread: ThreadId, turn: Option<Id>) {
        let mut state = self.state();
        if let Some(session) = state.sessions.get_mut(&thread) {
            session.running_turn = turn;
            self.ctx
                .ledger
                .turn_running(thread, session.opening_id, turn);
        }
    }

    // ---- reading what the adapter reports

    fn spawn_pump(self: &Arc<Self>, stream: EventStream, generation: u64) {
        let process = Arc::clone(self);
        tokio::spawn(async move { process.pump(stream, generation).await });
    }

    async fn pump(self: Arc<Self>, mut stream: EventStream, generation: u64) {
        loop {
            let Some((first, capacity)) = stream.recv_held().await else {
                break;
            };
            let mut batch = vec![first];
            let mut capacities = vec![capacity];
            // Keep admission charged until this bounded batch reaches the spool. Draining a live
            // producer into an unbounded Vec would undo the sink's byte and node backpressure.
            while batch.len() < orch_agent_contract::flow::QUEUE_ITEMS {
                let Ok((more, capacity)) = stream.try_recv_held() else {
                    break;
                };
                batch.push(more);
                capacities.push(capacity);
            }
            let current = self.deliver_all(coalesce(batch), generation).await;
            drop(capacities);
            if !current {
                return;
            }
        }
        if !self.stopping.load(Ordering::SeqCst)
            && self.generation.load(Ordering::SeqCst) == generation
        {
            let process = Arc::clone(&self);
            process
                .died(generation, "the harness stopped reporting".to_owned())
                .await;
        }
    }

    async fn keep_output_blobs(&self, event: &Event, generation: u64) -> bool {
        // The producer may be awaiting a bounded sink while holding the adapter call mutex.
        // This immutable process-generation snapshot keeps the consumer independent of that mutex.
        let directory = lock(&self.output_directory).clone();
        let mut warned = false;
        loop {
            if self.stopping.load(Ordering::SeqCst)
                || self.generation.load(Ordering::SeqCst) != generation
            {
                return false;
            }
            let upload =
                output_blobs::keep(&self.ctx.services.content, directory.as_deref(), event);
            tokio::pin!(upload);
            let result = loop {
                tokio::select! {
                    result = &mut upload => break result,
                    () = tokio::time::sleep(RESTART_FIRST_PAUSE) => {
                        if self.stopping.load(Ordering::SeqCst)
                            || self.generation.load(Ordering::SeqCst) != generation
                        {
                            return false;
                        }
                    }
                }
            };
            match result {
                Ok(()) => {
                    return self.generation.load(Ordering::SeqCst) == generation
                        && !self.stopping.load(Ordering::SeqCst);
                }
                Err(error) => {
                    if !warned {
                        tracing::warn!(%error, "retained outputs wait for the content store before publication");
                        warned = true;
                    }
                    tokio::time::sleep(RESTART_FIRST_PAUSE).await;
                }
            }
        }
    }

    /// Puts the events of the adapter that waited together on their streams, in their order, and takes note
    /// of what they say of the sessions. All are handed to the link before any answer is waited for
    /// (C-turns: one event to a turn of the link, each turn forcing the spool to disk, a harness of ten
    /// sessions fell three minutes behind what it said, and the turns it had begun were heard of too late
    /// and given up). Says whether the process is still the one they are of.
    async fn deliver_all(self: &Arc<Self>, events: Vec<AdapterEvent>, generation: u64) -> bool {
        let mut kept = Vec::with_capacity(events.len());
        let mut observations = Vec::new();
        let mut deaths = Vec::new();
        let mut current = true;
        for event in events {
            if self.generation.load(Ordering::SeqCst) != generation {
                current = false;
                break;
            }
            let mut death = None;
            let event = match event {
                AdapterEvent::Status(mut status) => {
                    // How many sessions are open is for the connector to say: it opened them.
                    status.sessions_open = u32::try_from(self.sessions_open()).unwrap_or(u32::MAX);
                    if status.harness_version.is_none() {
                        status.harness_version = Some(self.state().version.clone());
                    }
                    death = death_of(&status);
                    AdapterEvent::Status(status)
                }
                other => other,
            };
            self.observe_before_publication(&event);
            match convert(self.provider_id, self.agent_id, &event) {
                Ok(converted) => {
                    if !self.keep_output_blobs(&converted.event, generation).await {
                        current = false;
                        break;
                    }
                    if matches!(
                        &event,
                        AdapterEvent::Session {
                            event: SessionEvent::TurnCompleted(_)
                                | SessionEvent::SessionLost(_)
                                | SessionEvent::BackgroundTasks(_),
                            ..
                        }
                    ) {
                        observations.push(event);
                    }
                    kept.push(converted);
                }
                Err(_) => {
                    tracing::warn!("an event of the adapter could not be converted for publication")
                }
            }
            if let Some(reason) = death {
                deaths.push(reason);
            }
        }
        let mut warned = false;
        while !kept.is_empty() {
            let mut answers = self
                .ctx
                .services
                .publisher
                .publish_all(kept.clone())
                .await
                .into_iter();
            let mut retry = Vec::new();
            for event in kept {
                if !matches!(answers.next(), Some(Ok(()))) {
                    retry.push(event);
                }
            }
            kept = retry;
            if !kept.is_empty() {
                if !warned {
                    tracing::warn!(
                        "native output is held until its events can be published durably"
                    );
                    warned = true;
                }
                tokio::time::sleep(RESTART_FIRST_PAUSE).await;
            }
        }
        if self.generation.load(Ordering::SeqCst) == generation {
            for observation in observations {
                self.observe(&observation);
            }
        }
        for reason in deaths {
            let process = Arc::clone(self);
            tokio::spawn(async move { process.died(generation, reason).await });
        }
        current && self.generation.load(Ordering::SeqCst) == generation
    }

    /// Native running identity is retained immediately; terminal retirement waits for a durable receipt.
    fn observe_before_publication(&self, event: &AdapterEvent) {
        match event {
            AdapterEvent::Session {
                native_opening_id,
                thread_id,
                segment_id,
                event: SessionEvent::BackgroundTasks(report),
                ..
            } => {
                if let Some(session) = self.state().sessions.get_mut(thread_id)
                    && Some(session.opening_id) == *native_opening_id
                    && session.segment_id == *segment_id
                {
                    for task in &report.tasks {
                        if task.state
                            == orch_agent_contract::background::BackgroundTaskState::Running
                        {
                            session
                                .background
                                .insert(serde_json::json!([task.id, task.run_id]).to_string());
                        }
                    }
                    if report.self_waking && report.tasks.iter().any(|task| task.state.wakes()) {
                        session.awaiting_native = true;
                    }
                }
            }
            AdapterEvent::Login(_)
            | AdapterEvent::Session {
                event: SessionEvent::TurnStarted(_),
                ..
            } => self.observe(event),
            _ => {}
        }
    }

    fn observe(&self, event: &AdapterEvent) {
        if let AdapterEvent::Login(state) = event {
            self.state().login_asks = state.asks.clone().unwrap_or_default();
            return;
        }
        let AdapterEvent::Session {
            native_opening_id,
            thread_id,
            segment_id,
            event,
            ..
        } = event
        else {
            return;
        };
        let Some(opening_id) = *native_opening_id else {
            return;
        };
        match event {
            SessionEvent::BackgroundTasks(report) => {
                if let Some(session) = self.state().sessions.get_mut(thread_id)
                    && session.segment_id == *segment_id
                    && session.opening_id == opening_id
                {
                    for task in &report.tasks {
                        let key = serde_json::json!([task.id, task.run_id]).to_string();
                        if task.state
                            == orch_agent_contract::background::BackgroundTaskState::Running
                        {
                            session.background.insert(key);
                        } else {
                            session.background.remove(&key);
                        }
                    }
                    if report.self_waking && report.tasks.iter().any(|task| task.state.wakes()) {
                        session.awaiting_native = true;
                    }
                    if report.native_idle {
                        session.awaiting_native = false;
                    }
                }
            }
            SessionEvent::TurnStarted(started) => {
                if let Some(session) = self.state().sessions.get_mut(thread_id)
                    && session.segment_id == *segment_id
                    && session.opening_id == opening_id
                    && session
                        .running_turn
                        .is_none_or(|turn| turn == started.turn_id)
                {
                    session.running_turn = Some(started.turn_id);
                    session.awaiting_native = false;
                    self.ctx.ledger.turn_running(
                        *thread_id,
                        session.opening_id,
                        Some(started.turn_id),
                    );
                }
                self.ctx.ledger.turn_native(
                    *thread_id,
                    opening_id,
                    started.turn_id,
                    &started.native_turn_ref,
                );
            }
            SessionEvent::TurnCompleted(completed) => {
                if let Some(session) = self.state().sessions.get_mut(thread_id)
                    && session.segment_id == *segment_id
                    && session.opening_id == opening_id
                    && session.running_turn == Some(completed.turn_id)
                {
                    session.running_turn = None;
                    self.ctx.ledger.turn_running_if(
                        *thread_id,
                        session.opening_id,
                        Some(completed.turn_id),
                        None,
                    );
                }
                let commands = lock(&self.scratch).remove(&(Some(opening_id), completed.turn_id));
                if let Some(commands) = commands {
                    let content = Arc::clone(&self.ctx.services.content);
                    tokio::spawn(async move {
                        for command in commands {
                            content.finish(command).await;
                        }
                    });
                }
            }
            SessionEvent::SessionLost(lost) if lost.opening_id == opening_id => {
                self.session_closed_if(*thread_id, opening_id)
            }
            _ => {}
        }
    }

    // ---- dying and being started again

    /// The process is gone. Its sessions are lost, and it is started again if the rules allow.
    async fn died(self: Arc<Self>, generation: u64, reason: String) {
        if self.stopping.load(Ordering::SeqCst)
            || self
                .generation
                .compare_exchange(
                    generation,
                    generation + 1,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_err()
        {
            return;
        }
        tracing::warn!(provider = %self.provider_id, agent = %self.agent_id, %reason, "a harness process died");
        let lost: Vec<(ThreadId, OpenSession)> = {
            let mut state = self.state();
            state.status = HarnessState::Error;
            state.detail = Some(reason.clone());
            state.sessions.drain().collect()
        };
        for (thread, session) in lost {
            {
                let mut threads = lock(&self.ctx.threads);
                if threads.get(&thread) == Some(&(self.key(), session.opening_id)) {
                    threads.remove(&thread);
                }
            }
            self.ctx
                .ledger
                .session_closed_if(thread, session.opening_id);
            self.ctx.services.loopback.unbind(&session.session_key);
            let stream = Stream::Session {
                thread_id: thread,
                segment_id: session.segment_id,
            };
            let event = SessionEvent::SessionLost(orch_agent_contract::events::SessionLost {
                expected: false,
                opening_id: session.opening_id,
                uncertain: false,
                reason: format!("the harness process died: {reason}"),
            });
            if let Ok(body) = serde_json::to_value(&event)
                && let Ok(converted) = Event::decode(
                    event.kind(),
                    body.get("body").cloned().unwrap_or(Json::Null),
                )
            {
                self.publish_from(Some(session.opening_id), stream, converted)
                    .await;
            }
        }
        self.state().status = HarnessState::Error;
        self.state().detail = Some(reason.clone());
        self.publish_status(HarnessState::Error, Some(reason)).await;
        self.restart().await;
    }

    async fn restart(self: &Arc<Self>) {
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let attempts = {
                let mut state = self.state();
                let now = tokio::time::Instant::now();
                let window = self.ctx.window;
                state.restarts.retain(|at| now.duration_since(*at) < window);
                if u32::try_from(state.restarts.len()).unwrap_or(u32::MAX) >= self.ctx.restarts {
                    None
                } else {
                    state.restarts.push_back(now);
                    Some(state.restarts.len())
                }
            };
            let Some(attempts) = attempts else {
                let detail = format!(
                    "the harness died {} times within {} seconds; it stays failed until it is started again",
                    self.ctx.restarts,
                    self.ctx.window.as_secs()
                );
                self.state().detail = Some(detail.clone());
                self.publish_status(HarnessState::Error, Some(detail)).await;
                return;
            };
            let pause = RESTART_FIRST_PAUSE
                .saturating_mul(1u32 << (attempts.saturating_sub(1).min(5) as u32))
                .min(RESTART_LONGEST_PAUSE);
            self.state().status = HarnessState::Starting;
            self.publish_status(HarnessState::Starting, None).await;
            tokio::time::sleep(pause).await;
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            match self.relaunch().await {
                Ok(()) => return,
                Err(error) => {
                    tracing::warn!(%error, "a harness process could not be started again");
                    self.state().status = HarnessState::Error;
                    self.state().detail = Some(error.to_string());
                    self.publish_status(HarnessState::Error, Some(error.to_string()))
                        .await;
                }
            }
        }
    }

    async fn relaunch(self: &Arc<Self>) -> Result<(), CommandError> {
        let mut adapter = self.adapter.lock().await;
        let _ = adapter.stop().await;
        let (sink, stream) = EventSink::channel();
        let started = adapter
            .start(
                process_environment(&self.ctx.services, self.adapter_start.clone()),
                sink,
            )
            .await
            .map_err(|error| command_error(&error))?;
        let output_directory = adapter.output_directory();
        *lock(&self.interrupt) = adapter.interrupt_handle();
        drop(adapter);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *lock(&self.output_directory) = output_directory;
        {
            let mut state = self.state();
            state.status = HarnessState::Online;
            state.detail = None;
            state.version = started.harness_version;
            state.descriptor = Some(started.descriptor);
        }
        self.spawn_pump(stream, generation);
        self.publish_status(HarnessState::Online, None).await;
        self.publish_descriptor().await;
        Ok(())
    }

    /// Stops the process: its sessions end, its token is taken back, its record is forgotten.
    pub async fn stop(self: &Arc<Self>) {
        self.stopping.store(true, Ordering::SeqCst);
        self.generation.fetch_add(1, Ordering::SeqCst);
        let sessions: Vec<(ThreadId, OpenSession)> = self.state().sessions.drain().collect();
        for (thread, session) in &sessions {
            let mut threads = lock(&self.ctx.threads);
            if threads.get(thread) == Some(&(self.key(), session.opening_id)) {
                threads.remove(thread);
            }
        }
        self.ctx.services.loopback.revoke(self.grant.id);
        {
            let mut adapter = self.adapter.lock().await;
            if let Err(error) = adapter.stop().await {
                tracing::warn!(%error, "an adapter did not stop cleanly");
            }
        }
        self.ctx
            .ledger
            .harness_stopped(self.provider_id, self.agent_id);
        self.state().status = HarnessState::Offline;
        self.publish_status(HarnessState::Offline, None).await;
    }
}

// ---------------------------------------------------------------------------------------------------------
// All of them

/// All the harness processes of the connector.
pub struct Hosting {
    ctx: Arc<Context>,
    registry: Arc<AdapterRegistry>,
    processes: Mutex<HashMap<Key, Arc<Process>>>,
    orch_calls: Arc<crate::orch_tools::OrchCalls>,
    fetched: Mutex<BTreeMap<String, crate::builds::Installed>>,
}

impl std::fmt::Debug for Hosting {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Hosting").finish_non_exhaustive()
    }
}

impl Hosting {
    /// Hosting for a connector with these services, adapters and record.
    #[must_use]
    pub fn new(
        services: Arc<Services>,
        registry: Arc<AdapterRegistry>,
        ledger: Arc<Ledger>,
    ) -> Arc<Self> {
        let restarts = services.config.adapter.harness_restarts;
        let window = Duration::from_secs(services.config.adapter.harness_restart_window_secs);
        // The calls of Orch's tools kept after their answer did not come in time live in the same record.
        let orch_calls = Arc::new(crate::orch_tools::OrchCalls::with_ledger(Arc::clone(
            &ledger,
        )));
        Arc::new(Self {
            ctx: Arc::new(Context {
                services,
                ledger,
                threads: Mutex::new(HashMap::new()),
                mcp_changed: Arc::new(tokio::sync::Notify::new()),
                restarts,
                window,
            }),
            registry,
            processes: Mutex::new(HashMap::new()),
            fetched: Mutex::new(BTreeMap::new()),
            orch_calls,
        })
    }

    /// The calls of Orch's tools that harnesses made through the listener and that wait for the system's answer.
    #[must_use]
    pub fn orch_calls(&self) -> &Arc<crate::orch_tools::OrchCalls> {
        &self.orch_calls
    }

    /// The record of what is hosted.
    #[must_use]
    pub fn ledger(&self) -> &Arc<Ledger> {
        &self.ctx.ledger
    }

    /// The services the hosting was made with.
    #[must_use]
    pub fn services(&self) -> &Arc<Services> {
        &self.ctx.services
    }

    /// The registered adapters used for locally permitted discovery.
    pub(crate) fn registry(&self) -> &AdapterRegistry {
        &self.registry
    }

    /// The process for a provider and an agent profile.
    pub fn process(&self, provider_id: Id, agent_id: Id) -> Result<Arc<Process>, CommandError> {
        lock(&self.processes)
            .get(&(provider_id, agent_id))
            .cloned()
            .ok_or_else(|| {
                unavailable(
                    "no harness process is online for this provider and agent profile; agent.start comes first",
                )
            })
    }

    /// The process a thread's session is open in.
    pub fn process_of(&self, thread: ThreadId) -> Result<Arc<Process>, CommandError> {
        let key = lock(&self.ctx.threads).get(&thread).copied();
        match key {
            Some(((provider, agent), _)) => self.process(provider, agent),
            None => Err(CommandError::new(
                ErrorCode::SessionNotFound,
                "no session of that thread is open on this machine",
            )),
        }
    }

    /// Finds only the captured physical owner, including an older process awaiting its actual close.
    pub fn process_for_target(
        &self,
        target: &orch_link_contract::commands::SessionTarget,
    ) -> Option<Arc<Process>> {
        lock(&self.processes)
            .values()
            .find(|process| process.session_target(target.thread_id).as_ref() == Some(target))
            .cloned()
    }

    /// A newer admitted opening retires older local keys before its preparation can wait.
    pub fn retire_older_session_keys(&self, target: &orch_link_contract::commands::SessionTarget) {
        let Some(epoch) = target.opening_epoch else {
            return;
        };
        let processes = lock(&self.processes).values().cloned().collect::<Vec<_>>();
        for process in processes {
            let state = process.state();
            if let Some(session) = state.sessions.get(&target.thread_id)
                && session.opening_epoch < epoch
            {
                self.ctx.services.loopback.unbind(&session.session_key);
            }
        }
    }

    /// Finds the exact retained segment without substituting this thread's newest process.
    pub fn process_of_segment(
        &self,
        thread: ThreadId,
        segment: Id,
    ) -> Result<Arc<Process>, CommandError> {
        lock(&self.processes)
            .values()
            .find(|process| process.has_segment(thread, segment))
            .cloned()
            .ok_or_else(|| {
                CommandError::new(
                    ErrorCode::SessionNotFound,
                    "this exact retained native session is unavailable",
                )
            })
    }

    /// The processes that serve a provider, in the order of their agent profiles' identifiers: the same order
    /// each time, so that a choice among them is the same each time.
    #[must_use]
    pub fn processes_of(&self, provider_id: Id) -> Vec<Arc<Process>> {
        let mut found: Vec<(Id, Arc<Process>)> = lock(&self.processes)
            .iter()
            .filter(|((provider, _), _)| *provider == provider_id)
            .map(|((_, agent), process)| (*agent, Arc::clone(process)))
            .collect();
        found.sort_by_key(|(agent, _)| *agent);
        found.into_iter().map(|(_, process)| process).collect()
    }

    /// The processes of a provider whose adapter has a sign-in of its own, in the same order. Which of them is
    /// signed in only its adapter knows: a caller that needs the sign-in asks them in turn and passes over one
    /// that says it has none.
    #[must_use]
    pub fn processes_with_login(&self, provider_id: Id) -> Vec<Arc<Process>> {
        self.processes_of(provider_id)
            .into_iter()
            .filter(|process| {
                process
                    .started()
                    .1
                    .is_some_and(|descriptor| descriptor.native_provider.is_some())
            })
            .collect()
    }

    fn home_of(
        &self,
        request: &AgentStart,
        profile: &AgentProfile,
        registration: &AdapterRegistration,
    ) -> Result<Option<PathBuf>, CommandError> {
        let services = &self.ctx.services;
        let source = Self::home_source(request, profile);
        if request
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.environment_home)
        {
            if !services.policy.is_managed()
                || !services
                    .config
                    .connector
                    .roles
                    .iter()
                    .any(|role| role == "environment")
            {
                return Err(unavailable(
                    "this connector has no managed environment home",
                ));
            }
            if let Some(orch_agent_contract::facts::EnvironmentDataSource::ExistingDirectory {
                path,
            }) = &source
            {
                let path = PathBuf::from(path);
                crate::policy::writable_environment_directory(&path)?;
                return Ok(Some(path));
            }
            let root = std::env::var_os("ORCH_HOME_DIR")
                .map(PathBuf::from)
                .ok_or_else(|| {
                    unavailable("this environment has not reported its persistent home")
                })?;
            return Ok(Some(environment_connection_home(
                &root,
                request.provider_id,
                request.agent_id,
            )));
        }
        if let Ok(mapped) = std::env::var("ORCH_HARNESS_HOME") {
            let actual: orch_agent_contract::facts::EnvironmentDataSource =
                std::env::var("ORCH_HARNESS_HOME_SOURCE")
                    .ok()
                    .and_then(|value| serde_json::from_str(&value).ok())
                    .ok_or_else(|| {
                        unavailable(
                            "the runtime's harness home binding has no ownership declaration",
                        )
                    })?;
            if source.as_ref().is_some_and(|wanted| wanted != &actual) {
                return Err(unavailable(
                    "the harness home changed; apply the provider runtime update before starting it",
                ));
            }
            let path = PathBuf::from(mapped);
            crate::policy::writable_environment_directory(&path)?;
            return Ok(Some(path));
        }
        if let Some(orch_agent_contract::facts::EnvironmentDataSource::ExistingDirectory { path }) =
            &source
        {
            if !orch_envhost::RUNTIME_VERIFIED {
                return Err(unavailable(
                    "this native harness cannot retain a borrowed home through process retirement on this system; it can use its own home",
                ));
            }
            let policy=services.policy.policy().ok_or_else(||unavailable("an existing harness home needs a checked runtime binding; apply the provider runtime update"))?;
            let directory = crate::policy::inspect_environment_directory(&policy, Path::new(path))?;
            let path = PathBuf::from(directory.path);
            crate::policy::writable_environment_directory(&path)?;
            return Ok(Some(path));
        }
        if let Some(storage) = request
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.home_storage.as_deref())
            && let Some(policy) = services.policy.policy()
        {
            return crate::storages::directory(
                &services.data_dir,
                &services.config,
                &policy,
                request.provider_id,
                "home",
                Some(storage),
            )
            .map(Some);
        }
        if let Some(relative) = &request.home {
            let relative = Path::new(relative);
            if relative.is_absolute()
                || relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err(unavailable(
                    "the home is not a path inside the data directory",
                ));
            }
            return Ok(Some(services.data_dir.join(relative)));
        }
        Ok(
            (source.is_some() || registration.home_rule == HomeRule::Connector).then(|| {
                services
                    .data_dir
                    .join("harness")
                    .join(request.provider_id.to_string())
                    .join(request.agent_id.to_string())
            }),
        )
    }

    fn home_source(
        request: &AgentStart,
        profile: &AgentProfile,
    ) -> Option<orch_agent_contract::facts::EnvironmentDataSource> {
        request
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.home.clone())
            .or_else(|| {
                profile
                    .attach
                    .as_ref()
                    .and_then(|attach| attach.home.clone())
            })
            .or_else(|| {
                std::env::var("ORCH_HARNESS_HOME_SOURCE")
                    .ok()
                    .and_then(|value| serde_json::from_str(&value).ok())
            })
    }

    async fn binary_of(
        &self,
        request: &AgentStart,
        profile: &AgentProfile,
        registration: &AdapterRegistration,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Option<String>, CommandError> {
        let named = profile
            .attach
            .as_ref()
            .and_then(|attach| attach.binary.clone());
        if !registration.needs_binary {
            return Ok(named);
        }
        let Some(policy) = self.ctx.services.policy.policy() else {
            // A runtime the system made: the profile may name the program; otherwise it is the one the
            // node put in the tools directory for this kind of harness, and where there is none the
            // adapter looks on the path, as an image that brings the harness has it.
            return Ok(named.or_else(|| {
                let tools = self.ctx.services.tools_dir.as_deref()?;
                crate::builds::find_program(
                    tools,
                    &request.harness_kind,
                    &self.registry.programs_of(&request.harness_kind),
                )
                .map(|path| path.display().to_string())
            }));
        };
        if let Some(listed) = policy.harness(&request.harness_kind) {
            return match named {
                Some(wanted) if wanted != listed.path => Err(unavailable_by_policy(
                    orch_link_contract::policy::PolicyKey::AgentHarnesses,
                    &format!(
                        "the profile names {wanted}, which is not the program this machine lists for {}",
                        request.harness_kind
                    ),
                )),
                _ => Ok(Some(listed.path.clone())),
            };
        }
        if named.is_some() {
            return Err(unavailable_by_policy(
                orch_link_contract::policy::PolicyKey::AgentHarnesses,
                "confirm this local program on the machine's Policy page, or remove the profile's explicit program path",
            ));
        }
        if !policy.agent_programs {
            return Err(unavailable_by_policy(
                orch_link_contract::policy::PolicyKey::AgentPrograms,
                "allow program management on the machine's Policy page to use a system-listed harness build",
            ));
        }
        let cache = self.ctx.services.build_cache.as_ref().ok_or_else(|| unavailable(
            "this connector cannot fetch named harness builds; update its connector or confirm an installed program in agent.harnesses"
        ))?;
        let build = request
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.build.as_ref())
            .ok_or_else(|| {
                unavailable(
                    "no system-listed build was selected for this harness and machine platform",
                )
            })?;
        if build.name != request.harness_kind
            || build.job_id.is_some()
            || profile
                .harness_version
                .as_ref()
                .is_some_and(|pin| pin != &build.version)
        {
            return Err(unavailable(
                "the selected harness build differs from this profile's required program",
            ));
        }
        let directory = cache.ensure(build, &self.ctx.services, cancel).await?;
        if cancel.is_cancelled() {
            return Err(CommandError::new(
                ErrorCode::Timeout,
                "the harness start was cancelled",
            ));
        }
        if !self
            .ctx
            .services
            .policy
            .policy()
            .is_some_and(|policy| policy.agent_programs)
        {
            return Err(unavailable_by_policy(
                orch_link_contract::policy::PolicyKey::AgentPrograms,
                "program management was withdrawn on the machine's Policy page",
            ));
        }
        // The cache returns the selected version's directory, not an arbitrary program path.
        let parent = directory
            .parent()
            .ok_or_else(|| unavailable("the verified build has no tools directory"))?;
        let version = directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| unavailable("the verified build has no version directory"))?;
        let program = crate::builds::find_program(
            parent,
            version,
            &self.registry.programs_of(&request.harness_kind),
        )
        .ok_or_else(|| {
            unavailable(
                "the verified build contains no executable named by this harness's manifest",
            )
        })?;
        lock(&self.fetched).insert(
            request.harness_kind.clone(),
            crate::builds::Installed {
                kind: request.harness_kind.clone(),
                version: build.version.clone(),
                program: program.clone(),
            },
        );
        Ok(Some(program.display().to_string()))
    }

    /// Starts the process an `agent.start` asks for, or finds it running as asked.
    pub async fn start(
        self: &Arc<Self>,
        command: Option<Id>,
        request: &AgentStart,
    ) -> Result<AgentStartResult, CommandError> {
        self.start_cancellable(
            command,
            request,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
    }

    /// Command-owned acquisition and launch stop together when the command is cancelled.
    pub async fn start_cancellable(
        self: &Arc<Self>,
        command: Option<Id>,
        request: &AgentStart,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<AgentStartResult, CommandError> {
        let key = (request.provider_id, request.agent_id);
        let existing = lock(&self.processes).get(&key).cloned();
        let previous_group = lock(&self.processes)
            .values()
            .find(|process| {
                process.provider_id == request.provider_id && process.resource_group.is_some()
            })
            .and_then(|process| process.resource_group.clone());
        if let Some(process) = existing {
            let before = process.request();
            let same_runtime =
                if before.runtime == request.runtime {
                    true
                } else {
                    let same_home =
                        before.runtime.as_ref().map(|runtime| {
                            (&runtime.home, &runtime.home_storage, &runtime.resources)
                        }) == request.runtime.as_ref().map(|runtime| {
                            (&runtime.home, &runtime.home_storage, &runtime.resources)
                        });
                    if same_home {
                        let incoming = self.runtime_environment(command, request, false)?;
                        self.runtime_environment(None, &before, false)
                            .is_ok_and(|held| held == incoming)
                    } else {
                        false
                    }
                };
            let same = before.profile == request.profile
                && before.harness_kind == request.harness_kind
                && before.home == request.home
                && before.provider_config == request.provider_config
                && same_runtime
                && process.state().status == HarnessState::Online;
            if same {
                let described = process
                    .call(|adapter| Box::pin(async move { adapter.probe().await }))
                    .await;
                if let Ok(descriptor) = described {
                    process.state().descriptor = Some(descriptor);
                }
                let (version, descriptor) = process.started();
                return match descriptor {
                    Some(descriptor) => Ok(AgentStartResult {
                        home: Some(process.home_fact()),
                        harness_version: version,
                        descriptor,
                        reused: true,
                    }),
                    None => Err(unavailable("the harness did not say what it is")),
                };
            }
            process.stop().await;
            lock(&self.processes).remove(&key);
        }
        let registration = self.registry.get(&request.harness_kind).ok_or_else(|| {
            unavailable(format!(
                "this machine has no adapter for the harness kind {:?}",
                request.harness_kind
            ))
        })?;
        let mut profile: AgentProfile = request.profile.clone();
        let source = Self::home_source(request, &profile);
        let owned_home = source.as_ref().map_or(
            request.home.is_some() || registration.home_rule == HomeRule::Connector,
            |home| home.directory().is_none(),
        );
        let home = self.home_of(request, &profile, &registration)?;
        if let Some(source) = source {
            profile
                .attach
                .get_or_insert(orch_agent_contract::facts::Attach {
                    mode: Default::default(),
                    binary: None,
                    home: None,
                })
                .home = Some(source);
        }
        let mut binary = self
            .binary_of(request, &profile, &registration, cancel)
            .await?;
        profile
            .env
            .extend(self.runtime_environment(command, request, true)?);
        let services = &self.ctx.services;
        let resources = request
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.resources)
            .unwrap_or_default();
        let resource_group = if let Some(group) = previous_group {
            group
                .update(&resources, services.process_controls)
                .await
                .map_err(unavailable)?;
            Some(group)
        } else {
            orch_envhost::process_limits::UserGroup::open(
                request.provider_id,
                &resources,
                services.process_controls,
            )
            .await
            .map_err(unavailable)?
        };
        if let (Some(group), Some(program)) = (&resource_group, &binary) {
            binary = Some(
                group
                    .program(
                        &services
                            .data_dir
                            .join("resource-programs")
                            .join(request.provider_id.to_string()),
                        program,
                    )
                    .map_err(unavailable)?,
            );
        }
        let grant = services.loopback.grant();
        let adapter_start = process_environment(
            services,
            AdapterStart {
                provider_id: request.provider_id,
                provider_kind: request.provider_kind,
                agent_id: request.agent_id,
                harness_kind: request.harness_kind.clone(),
                profile,
                home: home.as_ref().map(|path| path.display().to_string()),
                // What the provider's owner configured, as the system sent it: what the page shows is what runs.
                provider_config: request.provider_config.clone(),
                loopback: AdapterLoopback::new(services.loopback.port(), grant.token.clone()),
                binary,
            },
        );
        let mut adapter = (registration.factory)(Arc::clone(&services.clock));
        let (sink, stream) = EventSink::channel();
        let started = match adapter.start(adapter_start.clone(), sink).await {
            Ok(started) => started,
            Err(error) => {
                services.loopback.revoke(grant.id);
                let last = !lock(&self.processes)
                    .values()
                    .any(|process| process.provider_id == request.provider_id);
                if last && let Some(group) = &resource_group {
                    group.close().await;
                }
                return Err(command_error(&error));
            }
        };
        let output_directory = adapter.output_directory();
        let interrupt = adapter.interrupt_handle();
        let process = Arc::new(Process {
            provider_id: request.provider_id,
            agent_id: request.agent_id,
            kind: request.harness_kind.clone(),
            ctx: Arc::clone(&self.ctx),
            request: Mutex::new(request.clone()),
            adapter: tokio::sync::Mutex::new(adapter),
            interrupt: Mutex::new(interrupt),
            output_directory: Mutex::new(output_directory),
            adapter_start,
            resource_group,
            owned_home,
            grant,
            state: Mutex::new(State {
                status: HarnessState::Online,
                detail: None,
                version: started.harness_version.clone(),
                descriptor: Some(started.descriptor.clone()),
                sessions: HashMap::new(),
                restarts: VecDeque::new(),
                login_asks: Vec::new(),
            }),
            stopping: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            scratch: Mutex::new(HashMap::new()),
        });
        self.ctx
            .ledger
            .harness_started(request, home.map(|path| path.display().to_string()));
        process.spawn_pump(stream, 0);
        lock(&self.processes).insert(key, Arc::clone(&process));
        process.publish_status(HarnessState::Online, None).await;
        process.publish_descriptor().await;
        Ok(AgentStartResult {
            home: Some(process.home_fact()),
            harness_version: started.harness_version,
            descriptor: started.descriptor,
            reused: false,
        })
    }

    /// Stops the process of a provider and an agent profile, and the sessions in it.
    pub async fn stop(&self, provider_id: Id, agent_id: Id) -> Result<(), CommandError> {
        let process = lock(&self.processes).remove(&(provider_id, agent_id));
        if let Some(process) = process {
            process.stop().await;
            let last = !lock(&self.processes)
                .values()
                .any(|other| other.provider_id == provider_id);
            if last && let Some(group) = &process.resource_group {
                group.close().await;
            }
        }
        crate::credentials::SecretStore::open(
            &self.ctx.services.data_dir,
            Arc::clone(&self.ctx.services.keys),
        )
        .and_then(|store| store.remove(&runtime_environment_name(provider_id, agent_id)))
        .map_err(|_| {
            unavailable("the stopped harness's runtime variables could not be forgotten")
        })?;
        Ok(())
    }

    /// Stops every process, for the connector's own stop.
    pub async fn stop_all(&self) {
        let processes: Vec<Arc<Process>> = lock(&self.processes).drain().map(|(_, p)| p).collect();
        for process in processes {
            // The record stays: what was running is what the next start picks up.
            process.stopping.store(true, Ordering::SeqCst);
            process.generation.fetch_add(1, Ordering::SeqCst);
            self.ctx.services.loopback.revoke(process.grant.id);
            let mut adapter = process.adapter.lock().await;
            let _ = adapter.stop().await;
            if let Some(group) = &process.resource_group {
                group.close().await;
            }
        }
    }

    /// Retire idle harnesses of a replaced external adapter before its supervisor is stopped.
    pub async fn stop_kind(&self, kind: &str) -> Result<(), CommandError> {
        let keys: Vec<_> = lock(&self.processes)
            .iter()
            .filter(|(_, process)| process.kind == kind)
            .map(|(key, _)| *key)
            .collect();
        for (provider, agent) in keys {
            self.stop(provider, agent).await?;
        }
        Ok(())
    }

    /// Lets go of every process at once, without stopping them or touching the record: what happens to
    /// a connector whose process is killed. For tests of what the next start finds.
    pub fn abandon(&self) {
        let processes: Vec<Arc<Process>> = lock(&self.processes).drain().map(|(_, p)| p).collect();
        for process in processes {
            process.stopping.store(true, Ordering::SeqCst);
            process.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// What the processes and the registry add to the machine's facts.
    pub fn contribute(&self, draft: &mut FactsDraft) {
        draft.adapters.extend(self.registry.facts());
        if self.ctx.services.build_cache.is_some() {
            draft.build_fetch = Some(orch_driver_contract::descriptor::OperationSupport {
                supported: true,
                reason: None,
            });
        }
        draft.harnesses.extend(
            lock(&self.fetched)
                .values()
                .filter(|program| {
                    !self.ctx.services.installed.iter().any(|installed| {
                        installed.kind == program.kind
                            && installed.version == program.version
                            && installed.program == program.program
                    })
                })
                .map(|program| orch_link_contract::session::MachineFactsHarness {
                    kind: program.kind.clone(),
                    path: program.program.display().to_string(),
                    version: program.version.clone(),
                    daemon: false,
                }),
        );
        if let Some(policy) = self.ctx.services.policy.policy() {
            draft
                .harnesses
                .extend(policy.agent_harnesses.iter().map(|harness| {
                    orch_link_contract::session::MachineFactsHarness {
                        kind: harness.kind.clone(),
                        path: harness.path.clone(),
                        version: "unknown".to_owned(),
                        daemon: false,
                    }
                }));
        }
    }

    /// What a previous run of the connector was hosting.
    #[must_use]
    pub fn recorded(&self) -> Vec<HostedHarness> {
        self.ctx.ledger.hosted()
    }

    /// Keeps the applied sandbox with its native command before returning its successful result.
    pub async fn report_sandbox(
        &self,
        session: (ThreadId, Id),
        turn: Option<Id>,
        command: Id,
        applied: SandboxMode,
        native_opening_id: Option<Id>,
        clamps: &[Clamp],
    ) -> Result<(), CommandError> {
        // Managed runtimes bypass this gate; their template's selection is not a machine-policy fact.
        if self.ctx.services.policy.policy().is_none() {
            return Ok(());
        }
        let (thread, segment) = session;
        let mode = |value: &str| {
            serde_json::from_value(Json::String(value.to_owned())).map_err(|_| {
                CommandError::new(
                    ErrorCode::Internal,
                    "The applied sandbox could not be recorded.",
                )
            })
        };
        let requested = clamps
            .iter()
            .find(|clamp| clamp.key == "agent.max_sandbox")
            .map_or(applied.as_str(), |clamp| clamp.asked.as_str());
        let resolution = orch_api_contract::threads::SandboxResolution {
            requested: mode(requested)?,
            applied: mode(applied.as_str())?,
            governing_policy: orch_api_contract::environment::SandboxLimitSource::MachinePolicy,
            command_id: command,
            at: self.ctx.services.clock.time(),
        };
        self.ctx
            .services
            .publisher
            .publish_from(
                native_opening_id,
                Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                Event::SessionSandbox(SessionSandbox {
                    turn_id: turn,
                    resolution,
                }),
            )
            .await
            .map_err(|_| {
                CommandError::new(
                    ErrorCode::Internal,
                    "The applied sandbox is waiting for durable publication.",
                )
                .retryable()
            })?;
        for clamp in clamps {
            tracing::warn!(key = clamp.key, asked = %clamp.asked, used = %clamp.used, "a request was lowered to the machine's ceiling");
            let frame = Event::RawFrame(RawFrame {
                text: Vec::new(),
                text_complete: true,
                n: 0,
                direction: "connector".to_owned(),
                payload: serde_json::json!({
                    "policy": clamp.key, "asked": clamp.asked, "used": clamp.used
                }),
            });
            if let Err(error) = self
                .ctx
                .services
                .publisher
                .publish_from(
                    native_opening_id,
                    Stream::Session {
                        thread_id: thread,
                        segment_id: segment,
                    },
                    frame,
                )
                .await
            {
                tracing::warn!(%error, "a diagnostic could not be put in the spool");
            }
        }
        Ok(())
    }

    /// The segment a thread's session serves, if it is open here.
    #[must_use]
    pub fn segment_of(&self, thread: ThreadId) -> Option<Id> {
        let (key, opening) = lock(&self.ctx.threads).get(&thread).copied()?;
        let process = lock(&self.processes).get(&key).cloned()?;
        let state = process.state();
        state
            .sessions
            .get(&thread)
            .filter(|session| session.opening_id == opening)
            .map(|session| session.segment_id)
    }

    /// Makes the binding of a new session in the connector's listener.
    pub fn bind(
        &self,
        process: &Process,
        target: &orch_link_contract::native_target::SessionTarget,
        tickets: Tickets,
    ) -> Result<Arc<SessionBinding>, CommandError> {
        self.ctx
            .services
            .loopback
            .bind_native(
                process.grant.id,
                process.provider_id,
                process.agent_id,
                target,
                tickets,
            )
            .ok_or_else(|| unavailable("the native binding has no physical target"))
    }

    /// Registers a process that was started again by [`crate::reconciliation`], with the record it had.
    pub(crate) async fn restart_recorded(
        self: &Arc<Self>,
        hosted: &HostedHarness,
    ) -> Result<Arc<Process>, CommandError> {
        self.start(None, &hosted.start).await?;
        self.process(hosted.start.provider_id, hosted.start.agent_id)
    }

    /// Secrets are opened only at this machine; the plain hosted record never receives an expanded profile.
    fn runtime_environment(
        &self,
        command: Option<Id>,
        request: &AgentStart,
        remember: bool,
    ) -> Result<BTreeMap<String, String>, CommandError> {
        let Some(runtime) = &request.runtime else {
            return Ok(BTreeMap::new());
        };
        let failed = || unavailable("the provider runtime's sealed variables are unavailable");
        let name = runtime_environment_name(request.provider_id, request.agent_id);
        let fingerprint = orch_types::Hash::of(&serde_json::to_vec(runtime).map_err(|_| failed())?);
        let store = crate::credentials::SecretStore::open(
            &self.ctx.services.data_dir,
            Arc::clone(&self.ctx.services.keys),
        )
        .map_err(|_| failed())?;
        let Some(command) = command else {
            let kept: RuntimeEnvironment =
                serde_json::from_str(&store.get(&name).map_err(|_| failed())?.ok_or_else(failed)?)
                    .map_err(|_| failed())?;
            if kept.fingerprint != fingerprint {
                return Err(failed());
            }
            return Ok(kept.variables);
        };
        let mut variables = BTreeMap::new();
        for (name, value) in &runtime.variables {
            if name.is_empty() || name.contains(['=', '\0']) || name.starts_with("ORCH_") {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "runtime variable names must be nonempty, contain no equals or NUL and leave ORCH_ names to the connector",
                ));
            }
            let value = match value {
                SecretOrText::Text(text) => text.clone(),
                SecretOrText::Secret(LinkSecret::Sealed(value)) => {
                    self.ctx.services.keys.open(command, value)?
                }
                SecretOrText::Secret(LinkSecret::Ref(_)) => return Err(failed()),
            };
            if value.contains('\0') {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "a runtime variable cannot contain NUL",
                ));
            }
            variables.insert(name.clone(), value);
        }
        let kept = RuntimeEnvironment {
            fingerprint,
            variables,
        };
        if remember {
            store
                .put(&name, &serde_json::to_string(&kept).map_err(|_| failed())?)
                .map_err(|_| failed())?;
        }
        Ok(kept.variables)
    }
}

impl Process {
    /// The actual running process's home and the choice that produced it.
    #[must_use]
    pub fn home_fact(&self) -> orch_driver_contract::runtime::HarnessHome {
        use orch_driver_contract::runtime::{HarnessHome, HarnessHomeSource};
        let request = lock(&self.request);
        let source = if request
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.home.as_ref())
            .is_some()
        {
            HarnessHomeSource::Runtime
        } else if request
            .profile
            .attach
            .as_ref()
            .and_then(|attach| attach.home.as_ref())
            .is_some()
        {
            HarnessHomeSource::Profile
        } else if self.adapter_start.home.is_some() {
            HarnessHomeSource::Managed
        } else {
            HarnessHomeSource::Native
        };
        HarnessHome {
            path: self.adapter_start.home.clone(),
            source,
        }
    }

    /// Whether the resolved home is a private home made by the connector, rather than a person's home.
    #[must_use]
    pub const fn home_owned(&self) -> bool {
        self.owned_home
    }

    /// Credential changes in a person's home use the same live machine policy as every other command.
    pub fn sign_in_allowed(&self) -> Result<(), CommandError> {
        // Unit connectors have no independent physical policy; the server admits them against their
        // hosting machine's live report. Every physical connector rechecks its own switch here.
        if self.owned_home
            || self
                .ctx
                .services
                .policy
                .policy()
                .is_none_or(|policy| policy.provider_signin)
        {
            return Ok(());
        }
        Err(CommandError::new(
            ErrorCode::PolicySignInNotAllowed,
            "provider.signin: this harness uses the person's own home; its machine has not permitted Orch to change or renew that sign-in",
        ))
    }

    /// Inspects only the exact admitted native turn; no model input, resume or interruption is submitted.
    pub async fn recover_recorded_turn(
        &self,
        session: &HostedSession,
    ) -> Result<bool, CommandError> {
        let Some(turn) = session.running_turn else {
            let native = session.native_session_ref.clone();
            self.call(move |adapter| {
                Box::pin(async move { adapter.read_session(&native, false).await })
            })
            .await?;
            return Ok(true);
        };
        let source = orch_agent_contract::history::NativePrefixSource {
            thread_id: session.thread_id,
            segment_id: session.segment_id,
            native_session_ref: session.native_session_ref.clone(),
        };
        let source = orch_agent_contract::history::RecoverySource {
            session: source,
            opening_id: session.opening_id,
        };
        let native = session.native_turn_ref.clone();
        let directory = session.directory.clone();
        let cwd = session.cwd.clone();
        let capture = session.output_capture_bytes;
        self.call(move |adapter| {
            Box::pin(async move {
                adapter
                    .recover_turn(source, turn, native, directory, cwd, capture)
                    .await
            })
        })
        .await
    }

    /// Reports a session of this process as lost, to the stream of its session.
    pub async fn report_lost(&self, thread: ThreadId, segment: Id, opening_id: Id, reason: &str) {
        let event = SessionEvent::SessionLost(orch_agent_contract::events::SessionLost {
            expected: false,
            opening_id,
            uncertain: false,
            reason: reason.to_owned(),
        });
        if let Ok(body) = serde_json::to_value(&event)
            && let Ok(converted) = Event::decode(
                event.kind(),
                body.get("body").cloned().unwrap_or(Json::Null),
            )
        {
            self.publish_from(
                Some(opening_id),
                Stream::Session {
                    thread_id: thread,
                    segment_id: segment,
                },
                converted,
            )
            .await;
        }
    }

    /// Reports the process's own state, for the first report after it was started again.
    pub async fn report_started(&self) {
        self.publish_status(HarnessState::Online, None).await;
    }
}

impl Hosting {
    /// Session changes wake native discovery without waiting for its periodic file check.
    #[must_use]
    pub fn mcp_changes(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.ctx.mcp_changed)
    }

    /// Only directories of local sessions actually attached to this environment are inspected here.
    #[must_use]
    pub fn mcp_directories(&self, environment: Id, root: &str) -> Vec<String> {
        lock(&self.processes)
            .values()
            .flat_map(|process| {
                process
                    .state()
                    .sessions
                    .values()
                    .flat_map(|session| session.configuration_directories(environment, root))
                    .collect::<Vec<_>>()
            })
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// The harness processes that are online and whose adapters can say which tool servers their harness
    /// has configured, in the order of their identifiers.
    #[must_use]
    pub fn with_mcp_discovery(&self) -> Vec<Arc<Process>> {
        let mut found: Vec<(Key, Arc<Process>)> = lock(&self.processes)
            .iter()
            .filter(|(_, process)| {
                let state = process.state();
                state.status == HarnessState::Online
                    && state
                        .descriptor
                        .as_ref()
                        .is_some_and(|descriptor| descriptor.capabilities.mcp_discovery)
            })
            .map(|(key, process)| (*key, Arc::clone(process)))
            .collect();
        found.sort_by_key(|(key, _)| *key);
        found.into_iter().map(|(_, process)| process).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_connections_keep_distinct_native_homes_on_the_environment_home_volume() {
        let provider = orch_types::new_id();
        let agent = orch_types::new_id();
        let root = Path::new("/persistent/environment-home");
        let home = environment_connection_home(root, provider, agent);
        assert!(home.starts_with(root));
        assert_eq!(home, environment_connection_home(root, provider, agent));
        assert_ne!(
            home,
            environment_connection_home(root, orch_types::new_id(), agent)
        );
        assert_ne!(
            home,
            environment_connection_home(root, provider, orch_types::new_id())
        );
        assert!(!home.starts_with("/connector-state"));
    }

    fn idle_lane() -> OpenSession {
        OpenSession {
            opening_id: orch_types::new_id(),
            opening_epoch: 1,
            native_environment: None,
            directory: String::new(),
            cwd: String::new(),
            segment_id: orch_types::new_id(),
            native_session_ref: "synthetic-native-source".into(),
            continuation_fenced: true,
            session_key: "synthetic-session-key".into(),
            running_turn: None,
            background: std::collections::HashSet::new(),
            awaiting_native: false,
        }
    }

    #[test]
    fn a_local_session_discovers_its_directory_chain_only_in_its_actual_environment() {
        let environment = orch_types::new_id();
        let mut lane = idle_lane();
        assert!(
            lane.configuration_directories(environment, "/workspace")
                .is_empty()
        );
        lane.native_environment = Some(environment);
        lane.directory = "/workspace/project".into();
        lane.cwd = "/workspace/project/api/src".into();
        assert_eq!(
            lane.configuration_directories(environment, "/workspace")
                .into_iter()
                .collect::<Vec<_>>(),
            [
                "/workspace",
                "/workspace/project",
                "/workspace/project/api",
                "/workspace/project/api/src"
            ]
        );
        assert!(
            lane.configuration_directories(orch_types::new_id(), "/workspace")
                .is_empty()
        );
        lane.native_environment = None;
        assert!(
            lane.configuration_directories(environment, "/workspace")
                .is_empty(),
            "a remote executor's directory is never inspected on the program's machine"
        );
    }

    #[test]
    fn a_late_start_or_close_refuses_a_different_segment_and_an_unfenced_call() {
        let mut lane = idle_lane();
        assert_eq!(
            lane.operation(Some(orch_types::new_id()))
                .err()
                .map(|error| error.code),
            Some("session.segment_changed".into())
        );
        assert_eq!(
            lane.operation(None).err().map(|error| error.code),
            Some("session.segment_changed".into())
        );
        assert!(lane.operation(Some(lane.segment_id)).is_ok());
        lane.continuation_fenced = false;
        assert!(
            lane.operation(None).is_ok(),
            "ordinary legacy sessions keep their existing command path"
        );
    }

    #[test]
    fn an_ordinary_refresh_cannot_replace_the_prepared_native_child() {
        let lane = idle_lane();
        assert!(
            lane.refresh(lane.segment_id, Some(&lane.native_session_ref))
                .is_ok()
        );
        for (segment, native) in [
            (orch_types::new_id(), Some(lane.native_session_ref.as_str())),
            (lane.segment_id, None),
            (lane.segment_id, Some("another-native-session")),
        ] {
            assert_eq!(
                lane.refresh(segment, native).err().map(|error| error.code),
                Some("session.segment_changed".into())
            );
        }
    }

    #[test]
    fn recovery_can_replace_only_its_actual_idle_candidate_or_fully_identified_retained_source() {
        let mut lane = idle_lane();
        let source = orch_agent_contract::history::NativePrefixSource {
            thread_id: orch_types::new_id(),
            segment_id: orch_types::new_id(),
            native_session_ref: "original-native-session".into(),
        };
        assert!(lane.replacement(&source, None, None).is_err());
        assert!(
            lane.replacement(&source, Some(orch_types::new_id()), None)
                .is_err()
        );
        assert_eq!(
            lane.replacement(&source, Some(lane.segment_id), None).ok(),
            Some(Some(lane.segment_id))
        );
        let retained = orch_agent_contract::history::NativePrefixSource {
            thread_id: source.thread_id,
            segment_id: lane.segment_id,
            native_session_ref: lane.native_session_ref.clone(),
        };
        assert_eq!(
            lane.replacement(&source, None, Some(&retained)).ok(),
            Some(Some(lane.segment_id))
        );
        let mut wrong = retained.clone();
        wrong.native_session_ref.push_str("-changed");
        assert!(lane.replacement(&source, None, Some(&wrong)).is_err());
        lane.running_turn = Some(orch_types::new_id());
        assert!(
            lane.replacement(&source, Some(lane.segment_id), Some(&retained))
                .is_err(),
            "even owned cleanup cannot replace running work"
        );
    }

    fn status(state: HarnessState, detail: Option<&str>) -> HarnessStatus {
        HarnessStatus {
            home: None,
            status: state,
            detail: detail.map(str::to_owned),
            harness_version: None,
            sessions_open: 0,
        }
    }

    #[test]
    fn a_harness_that_failed_or_whose_process_was_lost_is_dead_and_one_stopped_on_purpose_is_not() {
        assert_eq!(
            death_of(&status(HarnessState::Error, Some("it crashed"))).as_deref(),
            Some("it crashed")
        );
        assert_eq!(
            death_of(&status(HarnessState::Error, None)).as_deref(),
            Some("the harness process is gone")
        );
        assert_eq!(
            death_of(&status(
                HarnessState::Offline,
                Some("the plugin's process was lost")
            ))
            .as_deref(),
            Some("the plugin's process was lost"),
            "a process that was lost says why"
        );
        assert_eq!(
            death_of(&status(HarnessState::Offline, None)),
            None,
            "the adapter stopped it itself, to start it again with a sign-in it was given"
        );
        assert_eq!(death_of(&status(HarnessState::Online, None)), None);
    }
}
