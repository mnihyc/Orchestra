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
    AgentStart, CommandError, ErrorCode, Secret as LinkSecret, SecretOrText,
};
use orch_link_contract::events::{Event, RawFrame};
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

type Key = (Id, Id);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------------------------
// A process

struct OpenSession {
    segment_id: Id,
    native_session_ref: String,
    continuation_fenced: bool,
    session_key: String,
    running_turn: Option<Id>,
}

impl OpenSession {
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
    threads: Mutex<HashMap<ThreadId, Key>>,
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
    adapter_start: AdapterStart,
    authorized_program: Option<String>,
    resource_group: Option<orch_envhost::process_limits::UserGroup>,
    owned_home: bool,
    grant: Grant,
    state: Mutex<State>,
    stopping: AtomicBool,
    generation: AtomicU64,
    /// The commands whose files a turn is still using, by the turn.
    scratch: Mutex<HashMap<Id, Vec<Id>>>,
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
    /// A running local harness may start new work only while its original program is still allowed.
    pub(crate) fn check_program_policy(&self) -> Result<(), CommandError> {
        if let Some(program) = &self.authorized_program {
            let current = self.ctx.services.policy.harness(&self.kind);
            if current.as_ref().map(|harness| harness.path.as_str()) != Some(program.as_str()) {
                return Err(unavailable(format!(
                    "agent.harnesses: the program for {} is no longer confirmed on this machine",
                    self.kind
                )));
            }
        }
        Ok(())
    }
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
    pub fn keep_for_turn(&self, turn: Id, command: Id) {
        lock(&self.scratch).entry(turn).or_default().push(command);
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

    /// Runs one call of the adapter. Calls are one at a time: the adapter is `&mut`.
    pub async fn call<R>(
        &self,
        work: impl for<'a> FnOnce(&'a mut dyn AgentAdapter) -> BoxFuture<'a, Result<R, AdapterError>>,
    ) -> Result<R, CommandError> {
        if self.state().status != HarnessState::Online {
            return Err(unavailable(
                "the harness process is not running; it is being started again, or gave up",
            ));
        }
        let mut adapter = self.adapter.lock().await;
        work(adapter.as_mut())
            .await
            .map_err(|error| command_error(&error))
    }

    async fn publish(&self, stream: Stream, event: Event) {
        if let Err(error) = self.ctx.services.publisher.publish(stream, event).await {
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

    /// A session opened in the adapter: it is kept, in the process and on disk.
    pub fn session_opened(&self, binding: &SessionBinding, native_session_ref: &str) {
        self.state().sessions.insert(
            binding.thread_id,
            OpenSession {
                segment_id: binding.segment_id,
                native_session_ref: native_session_ref.to_owned(),
                continuation_fenced: false,
                session_key: binding.session_key.clone(),
                running_turn: None,
            },
        );
        lock(&self.ctx.threads).insert(binding.thread_id, self.key());
        self.ctx.ledger.session_opened(
            self.provider_id,
            self.agent_id,
            HostedSession {
                thread_id: binding.thread_id,
                segment_id: binding.segment_id,
                native_session_ref: native_session_ref.to_owned(),
                running_turn: None,
            },
        );
    }

    /// A session ended: everything kept of it goes.
    pub fn session_closed(&self, thread: ThreadId) {
        let removed = self.state().sessions.remove(&thread);
        lock(&self.ctx.threads).remove(&thread);
        self.ctx.ledger.session_closed(thread);
        if let Some(session) = removed {
            self.ctx.services.loopback.unbind(&session.session_key);
        }
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
        let removed = state.sessions.remove(&source.thread_id);
        let mut threads = lock(&self.ctx.threads);
        if threads.get(&source.thread_id) == Some(&self.key()) {
            threads.remove(&source.thread_id);
            self.ctx.ledger.session_closed(source.thread_id);
        }
        drop(threads);
        drop(state);
        if let Some(session) = removed {
            self.ctx.services.loopback.unbind(&session.session_key);
        }
    }

    /// Reserves a just-admitted turn before the adapter can return ahead of its TurnStarted event.
    pub fn reserve_turn(&self, thread: ThreadId, turn: Option<Id>) {
        let mut state = self.state();
        if let Some(session) = state.sessions.get_mut(&thread) {
            session.running_turn = turn;
            self.ctx.ledger.turn_running(thread, turn);
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

    /// Puts the events of the adapter that waited together on their streams, in their order, and takes note
    /// of what they say of the sessions. All are handed to the link before any answer is waited for
    /// (C-turns: one event to a turn of the link, each turn forcing the spool to disk, a harness of ten
    /// sessions fell three minutes behind what it said, and the turns it had begun were heard of too late
    /// and given up). Says whether the process is still the one they are of.
    async fn deliver_all(self: &Arc<Self>, events: Vec<AdapterEvent>, generation: u64) -> bool {
        let mut kept = Vec::with_capacity(events.len());
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
            self.observe(&event);
            match convert(self.provider_id, self.agent_id, &event) {
                Ok(converted) => kept.push(converted),
                Err(error) => {
                    tracing::warn!(%error, "an event of the adapter was not given to the link")
                }
            }
            if let Some(reason) = death {
                deaths.push(reason);
            }
        }
        for answer in self.ctx.services.publisher.publish_all(kept).await {
            if let Err(error) = answer {
                tracing::warn!(%error, "an event could not be put in the spool");
            }
        }
        for reason in deaths {
            let process = Arc::clone(self);
            tokio::spawn(async move { process.died(generation, reason).await });
        }
        current
    }

    fn observe(&self, event: &AdapterEvent) {
        if let AdapterEvent::Login(state) = event {
            self.state().login_asks = state.asks.clone().unwrap_or_default();
            return;
        }
        let AdapterEvent::Session {
            thread_id, event, ..
        } = event
        else {
            return;
        };
        match event {
            SessionEvent::TurnStarted(started) => {
                if let Some(session) = self.state().sessions.get_mut(thread_id) {
                    session.running_turn = Some(started.turn_id);
                }
                self.ctx
                    .ledger
                    .turn_running(*thread_id, Some(started.turn_id));
            }
            SessionEvent::TurnCompleted(completed) => {
                if let Some(session) = self.state().sessions.get_mut(thread_id) {
                    session.running_turn = None;
                }
                self.ctx.ledger.turn_running(*thread_id, None);
                let commands = lock(&self.scratch).remove(&completed.turn_id);
                if let Some(commands) = commands {
                    let content = Arc::clone(&self.ctx.services.content);
                    tokio::spawn(async move {
                        for command in commands {
                            content.finish(command).await;
                        }
                    });
                }
            }
            SessionEvent::SessionLost(_) => self.session_closed(*thread_id),
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
        let lost: Vec<(ThreadId, OpenSession)> = self.state().sessions.drain().collect();
        for (thread, session) in lost {
            lock(&self.ctx.threads).remove(&thread);
            self.ctx.ledger.session_closed(thread);
            self.ctx.services.loopback.unbind(&session.session_key);
            let stream = Stream::Session {
                thread_id: thread,
                segment_id: session.segment_id,
            };
            let event = SessionEvent::SessionLost(orch_agent_contract::events::SessionLost {
                reason: format!("the harness process died: {reason}"),
            });
            if let Ok(body) = serde_json::to_value(&event)
                && let Ok(converted) = Event::decode(
                    event.kind(),
                    body.get("body").cloned().unwrap_or(Json::Null),
                )
            {
                self.publish(stream, converted).await;
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
        self.check_program_policy()?;
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
        drop(adapter);
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
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
        for (thread, _) in &sessions {
            lock(&self.ctx.threads).remove(thread);
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
                restarts,
                window,
            }),
            registry,
            processes: Mutex::new(HashMap::new()),
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
            Some((provider, agent)) => self.process(provider, agent),
            None => Err(CommandError::new(
                ErrorCode::SessionNotFound,
                "no session of that thread is open on this machine",
            )),
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

    fn binary_of(
        &self,
        request: &AgentStart,
        profile: &AgentProfile,
        registration: &AdapterRegistration,
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
        let listed = policy.harness(&request.harness_kind).ok_or_else(|| {
            unavailable(format!(
                "agent.harnesses: this machine lists no program for {}",
                request.harness_kind
            ))
        })?;
        match named {
            Some(wanted) if wanted != listed.path => Err(unavailable(format!(
                "agent.harnesses: the profile names {wanted}, which is not the program this machine lists for {}",
                request.harness_kind
            ))),
            _ => Ok(Some(listed.path.clone())),
        }
    }

    /// Starts the process an `agent.start` asks for, or finds it running as asked.
    pub async fn start(
        self: &Arc<Self>,
        command: Option<Id>,
        request: &AgentStart,
    ) -> Result<(String, AgentDescriptor, bool), CommandError> {
        let registration = self.registry.get(&request.harness_kind).ok_or_else(|| {
            unavailable(format!(
                "this machine has no adapter for the harness kind {:?}",
                request.harness_kind
            ))
        })?;
        let selected_program = self.binary_of(request, &request.profile, &registration)?;
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
                && process
                    .authorized_program
                    .as_ref()
                    .is_none_or(|program| selected_program.as_ref() == Some(program))
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
                    Some(descriptor) => Ok((version, descriptor, true)),
                    None => Err(unavailable("the harness did not say what it is")),
                };
            }
            process.stop().await;
            lock(&self.processes).remove(&key);
        }
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
        let mut binary = selected_program;
        let authorized_program =
            if registration.needs_binary && self.ctx.services.policy.policy().is_some() {
                binary.clone()
            } else {
                None
            };
        profile
            .env
            .extend(self.runtime_environment(command, request, false)?);
        if authorized_program.as_deref().is_some_and(|program| {
            let path = Path::new(program);
            !path.is_absolute() && path.components().count() == 1
        }) {
            let machine_path = std::env::var_os("PATH");
            if profile.env.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("PATH")
                    && Some(std::ffi::OsStr::new(value)) != machine_path.as_deref()
            }) {
                return Err(unavailable(
                    "agent.harnesses: a bare program name cannot use a different PATH from the machine's policy",
                ));
            }
        }
        if command.is_some() && request.runtime.is_some() {
            self.runtime_environment(command, request, true)?;
        }
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
        let process = Arc::new(Process {
            provider_id: request.provider_id,
            agent_id: request.agent_id,
            kind: request.harness_kind.clone(),
            ctx: Arc::clone(&self.ctx),
            request: Mutex::new(request.clone()),
            adapter: tokio::sync::Mutex::new(adapter),
            adapter_start,
            authorized_program,
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
        Ok((started.harness_version, started.descriptor, false))
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

    /// Reports that a clamp of the machine's ceilings lowered what a session was asked to be, as a
    /// diagnostic of the session's stream.
    pub async fn report_clamps(&self, thread: ThreadId, segment: Id, clamps: &[Clamp]) {
        for clamp in clamps {
            tracing::warn!(key = clamp.key, asked = %clamp.asked, used = %clamp.used, "a request was lowered to the machine's ceiling");
            let frame = Event::RawFrame(RawFrame {
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
                .publish(
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
    }

    /// The segment a thread's session serves, if it is open here.
    #[must_use]
    pub fn segment_of(&self, thread: ThreadId) -> Option<Id> {
        let key = lock(&self.ctx.threads).get(&thread).copied()?;
        let process = lock(&self.processes).get(&key).cloned()?;
        let state = process.state();
        state
            .sessions
            .get(&thread)
            .map(|session| session.segment_id)
    }

    /// Makes the binding of a new session in the connector's listener.
    pub fn bind(
        &self,
        process: &Process,
        thread: ThreadId,
        segment: Id,
        tickets: Tickets,
    ) -> Arc<SessionBinding> {
        self.ctx.services.loopback.bind(
            process.grant.id,
            thread,
            segment,
            process.provider_id,
            process.agent_id,
            tickets,
        )
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

    /// Lists the sessions the adapter holds, by their native references.
    pub async fn native_refs(&self) -> Option<Vec<String>> {
        let capable = self
            .state()
            .descriptor
            .as_ref()
            .is_some_and(|descriptor| descriptor.capabilities.discover_sessions);
        if !capable {
            return None;
        }
        self.call(|adapter| Box::pin(async move { adapter.discover_sessions().await }))
            .await
            .ok()
            .map(|sessions| {
                sessions
                    .into_iter()
                    .map(|session| session.native_session_ref)
                    .collect()
            })
    }

    /// Reports a session of this process as lost, to the stream of its session.
    pub async fn report_lost(&self, thread: ThreadId, segment: Id, reason: &str) {
        let event = SessionEvent::SessionLost(orch_agent_contract::events::SessionLost {
            reason: reason.to_owned(),
        });
        if let Ok(body) = serde_json::to_value(&event)
            && let Ok(converted) = Event::decode(
                event.kind(),
                body.get("body").cloned().unwrap_or(Json::Null),
            )
        {
            self.publish(
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

    fn idle_lane() -> OpenSession {
        OpenSession {
            segment_id: orch_types::new_id(),
            native_session_ref: "synthetic-native-source".into(),
            continuation_fenced: true,
            session_key: "synthetic-session-key".into(),
            running_turn: None,
        }
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
