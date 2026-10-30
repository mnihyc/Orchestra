//! Ping, restart, reconnect, logs and shutdown.
//!
//! A [`Plugin`] is one registered implementation, kept for as long as its host runs. Starting it starts a
//! supervisor, a task that opens the plugin (starts the program or attaches to it), negotiates, serves, and when
//! the plugin is gone opens it again. What the supervisor does at each way a plugin can fail:
//!
//! | The plugin | The supervisor |
//! |------------|----------------|
//! | cannot be started or attached to, or does not answer `hello` in time | counts it as a failure: the registration's policy decides, and after a pause that doubles the next try is made |
//! | answers `hello` and is refused (another kind, no common version, a required method missing, a descriptor that cannot be used) | stops it for good, and lists it as failed with the reason |
//! | exits, or closes its output | fails the requests that were waiting, then the policy decides: `never` leaves it down, `on-failure` starts it again unless it exited by itself with success, `always` starts it again; an attached plugin is always attached to again |
//! | stays silent for 30 s and does not answer a ping for 10 s | kills it (a program that was started) or lets go of it (one that was attached to), and goes on as for an exit |
//! | does not answer a request in its deadline | fails that request as `timeout` and tells the plugin to stop working on it |
//! | writes lines that are no message | skips them and counts them |
//! | writes to its standard error | writes the lines to the host's log, apart from the host's own lines |
//!
//! The pause before a new try doubles from a second to a minute ([`backoff`]). It starts over from a second after
//! a session that lasted as long as the longest pause: a program that is repaired is back within a minute, and one
//! that dies as soon as it has said hello is not started every second.
//!
//! While a plugin is down, a call fails as unavailable ([`CallError::Unavailable`]) with the plugin's name; whatever
//! depends on it reports that, and nothing else is affected. [`Plugin::report`] says how it is doing, for
//! Management and for the metric `orch_plugins`.
//!
//! A host that stops calls [`Plugin::shutdown`]: a program that was started is told `shutdown`, its input is closed,
//! and it has ten seconds to finish and exit before it is killed. A service that was attached to is only let go of:
//! it belongs to whoever runs it, who is not told to stop it by a host that restarts.
//!
//! Time is the clock the host was given, and the supervisor looks at it every [`Timing::tick`]; nothing in it
//! sleeps for as long as a deadline.

use std::any::{Any, TypeId};
use std::collections::BTreeMap;
use std::future::pending;
use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use orch_plugin_contract::lifecycle::{SHUTDOWN_WITHIN, deadline};
use orch_plugin_contract::messages::ToPlugin;
use orch_plugin_contract::negotiation::Negotiated;
use orch_plugin_contract::registration::{
    BACKOFF_MOST, Ended, PluginRegistration, PluginStart, RegistrationError, backoff,
};
use orch_plugin_contract::{PluginKind, PluginState, PluginStatus};
use orch_types::{Clock, Json, SystemClock, Time};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;

use crate::negotiation::{Failure, Handshake, Refusal, hello_deadline};
use crate::requests::{Call, CallError, Connection, Counters, Observer, ObserverSlot, Verdict};
use crate::transport::{self, Line, LineReader, Process};

/// How often the supervisor looks at the clock: a quarter of a second. The shortest wait that is judged is the
/// answer to a ping, ten seconds, so it is judged to within two and a half percent; an idle plugin costs four
/// wake-ups a second.
pub const DEFAULT_TICK: Duration = Duration::from_millis(250);

/// How long the lines a program wrote before it ended are read after it has: one second, of the machine's time.
/// They are all in the pipe already; the bound is for a program that left something behind which still holds the
/// pipe open.
const DRAIN_WITHIN: Duration = Duration::from_secs(1);

/// How the supervisor keeps time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How often the clock is looked at.
    pub tick: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { tick: DEFAULT_TICK }
    }
}

/// A clock that never goes back: it starts at the time of the clock it is made from and then follows the
/// machine's monotonic clock, so that a correction of the system time is not taken for a plugin that was silent.
#[derive(Debug, Clone, Copy)]
pub struct MonotonicClock {
    start: DateTime<Utc>,
    began: Instant,
}

impl MonotonicClock {
    /// A clock that shows what `from` shows now, and goes on from there.
    #[must_use]
    pub fn starting_with(from: &dyn Clock) -> Self {
        Self {
            start: from.now(),
            began: Instant::now(),
        }
    }
}

impl Clock for MonotonicClock {
    fn now(&self) -> DateTime<Utc> {
        chrono::Duration::from_std(self.began.elapsed())
            .ok()
            .and_then(|elapsed| self.start.checked_add_signed(elapsed))
            .unwrap_or(self.start)
    }
}

/// What a plugin is started with.
#[derive(Debug, Clone)]
pub struct Options {
    /// The version of the host component, which `hello` carries.
    pub host_version: String,
    /// Where the time comes from.
    pub clock: Arc<dyn Clock>,
    /// How the supervisor keeps time.
    pub timing: Timing,
}

impl Options {
    /// Options for a host of version `host_version`, on a clock that follows the machine's monotonic one.
    #[must_use]
    pub fn new(host_version: impl Into<String>) -> Self {
        Self {
            host_version: host_version.into(),
            clock: Arc::new(MonotonicClock::starting_with(&SystemClock)),
            timing: Timing::default(),
        }
    }

    /// The same options on another clock.
    #[must_use]
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The same options with other timing.
    #[must_use]
    pub fn with_timing(mut self, timing: Timing) -> Self {
        self.timing = timing;
        self
    }
}

/// The versions that were negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Versions {
    /// The version of the transport that is spoken.
    pub transport: u32,
    /// The version of the kind's contract that is spoken.
    pub contract: u32,
}

/// How a plugin is doing, as Management lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The extension point it implements.
    pub kind: PluginKind,
    /// The name it is registered by.
    pub name: String,
    /// Its own version, once it has said it; empty before.
    pub version: String,
    /// How it is doing.
    pub state: PluginState,
    /// Why it is not running, or how it came to be started again; the last error.
    pub detail: Option<String>,
    /// How many times it was started or attached to.
    pub starts: u64,
    /// How many lines of it were skipped because they were no message.
    pub malformed_lines: u64,
    /// The versions in force, while it runs and after it has run.
    pub versions: Option<Versions>,
    /// The optional methods of its kind that it does not implement: what is unavailable because of it.
    pub unavailable: Vec<String>,
    /// When it is started or attached to again, while it waits to be.
    pub retry_at: Option<Time>,
    /// Whether the first attempt to start it has come to an end, one way or the other.
    pub settled: bool,
}

impl Report {
    /// The part of the report that machine facts carry.
    #[must_use]
    pub fn status(&self) -> PluginStatus {
        PluginStatus {
            kind: self.kind,
            name: self.name.clone(),
            version: self.version.clone(),
            state: self.state,
        }
    }
}

/// The state as a metric label.
fn label(state: PluginState) -> &'static str {
    match state {
        PluginState::Running => "running",
        PluginState::Restarting => "restarting",
        PluginState::Failed => "failed",
    }
}

fn gauge(kind: PluginKind, state: PluginState) -> metrics::Gauge {
    metrics::gauge!("orch_plugins", "kind" => kind.as_str(), "state" => label(state))
}

#[derive(Default)]
struct Live {
    connection: Option<Arc<Connection>>,
    negotiated: Option<Arc<Negotiated>>,
}

struct Shared {
    kind: PluginKind,
    name: String,
    start: PluginStart,
    options: Options,
    live: Mutex<Live>,
    report: watch::Sender<Report>,
    observer: Arc<ObserverSlot>,
    counters: Arc<Counters>,
    stop: watch::Sender<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
    retired: AtomicBool,
    states: Mutex<BTreeMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl Shared {
    fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Edits the report, and moves the metric when the state moved.
    fn change(&self, edit: impl FnOnce(&mut Report)) {
        let mut moved = None;
        self.report.send_modify(|report| {
            let before = report.state;
            edit(report);
            if report.state != before {
                moved = Some((before, report.state));
            }
        });
        if let Some((before, after)) = moved
            && !self.retired.load(Ordering::Relaxed)
        {
            gauge(self.kind, before).decrement(1.0);
            gauge(self.kind, after).increment(1.0);
        }
    }

    /// The plugin is no longer hosted: it no longer counts in the metric.
    fn retire(&self) {
        if !self.retired.swap(true, Ordering::Relaxed) {
            let state = self.report.borrow().state;
            gauge(self.kind, state).decrement(1.0);
        }
    }

    fn unavailable(&self) -> CallError {
        let report = self.report.borrow();
        CallError::Unavailable {
            plugin: self.name.clone(),
            detail: report
                .detail
                .clone()
                .unwrap_or_else(|| format!("it is {}", label(report.state))),
        }
    }
}

/// Tells the supervisor to stop when the last handle of its plugin is gone.
struct StopOnDrop(Arc<Shared>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        // Nobody can ask for anything any more; a program that was started is told to finish.
        let _ = self.0.stop.send(true);
        self.0.retire();
    }
}

/// One registered implementation, started and kept. Cloning gives another handle to the same plugin.
#[derive(Clone)]
pub struct Plugin {
    shared: Arc<Shared>,
    _guard: Arc<StopOnDrop>,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin")
            .field("kind", &self.shared.kind)
            .field("name", &self.shared.name)
            .finish_non_exhaustive()
    }
}

impl Plugin {
    /// Registers `registration` and starts its supervisor on the current runtime. The first attempt to start the
    /// plugin is made at once; [`Plugin::settled`] waits for its outcome.
    pub async fn start(
        registration: &PluginRegistration,
        options: Options,
    ) -> Result<Self, RegistrationError> {
        let start = registration.start()?;
        let (report, _) = watch::channel(Report {
            kind: registration.kind,
            name: registration.name.clone(),
            version: String::new(),
            state: PluginState::Restarting,
            detail: None,
            starts: 0,
            malformed_lines: 0,
            versions: None,
            unavailable: Vec::new(),
            retry_at: None,
            settled: false,
        });
        let (stop, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            kind: registration.kind,
            name: registration.name.clone(),
            start,
            options,
            live: Mutex::new(Live::default()),
            report,
            observer: Arc::new(ObserverSlot::default()),
            counters: Arc::new(Counters::default()),
            stop,
            task: Mutex::new(None),
            retired: AtomicBool::new(false),
            states: Mutex::new(BTreeMap::new()),
        });
        gauge(shared.kind, PluginState::Restarting).increment(1.0);
        let task = tokio::spawn(supervise(Arc::clone(&shared)));
        *shared.task.lock().unwrap_or_else(PoisonError::into_inner) = Some(task);
        Ok(Self {
            _guard: Arc::new(StopOnDrop(Arc::clone(&shared))),
            shared,
        })
    }

    /// The extension point it implements.
    #[must_use]
    pub fn kind(&self) -> PluginKind {
        self.shared.kind
    }

    /// The name it is registered by.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.shared.name
    }

    /// Whether two handles supervise the same live registration, rather than equal commands.
    #[must_use]
    pub fn same_instance(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// How it is doing now.
    #[must_use]
    pub fn report(&self) -> Report {
        let mut report = self.shared.report.borrow().clone();
        report.malformed_lines = self.shared.counters.malformed_lines();
        report
    }

    /// A receiver that is woken whenever the report changes.
    #[must_use]
    pub fn watch(&self) -> watch::Receiver<Report> {
        self.shared.report.subscribe()
    }

    /// Waits until the first attempt to start the plugin has come to an end: it runs, or it has failed.
    pub async fn settled(&self) {
        let mut reports = self.shared.report.subscribe();
        // The sender lives in the shared state this handle holds, so the wait cannot be cut short.
        let _ = reports.wait_for(|report| report.settled).await;
    }

    /// What was negotiated the last time the plugin said hello; `None` if it never did.
    #[must_use]
    pub fn negotiated(&self) -> Option<Arc<Negotiated>> {
        self.shared.live().negotiated.clone()
    }

    /// Whether the plugin declared that it implements `method`.
    #[must_use]
    pub fn supports(&self, method: &str) -> bool {
        self.negotiated()
            .is_some_and(|negotiated| negotiated.methods.supports(method))
    }

    /// The state of type `T` that belongs to this plugin and is shared by everything that is made from it, made
    /// when it is first asked for. The bridges of one adapter share what they need to route its events this way.
    pub(crate) fn shared_state<T: Default + Send + Sync + 'static>(&self) -> Arc<T> {
        let mut states = self
            .shared
            .states
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let state = states
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(T::default()));
        Arc::clone(state)
            .downcast::<T>()
            // The state is kept under the identity of its type, so this is never reached.
            .unwrap_or_else(|_| Arc::new(T::default()))
    }

    /// Lets `observer` hear the plugin's events and progress, and when it is lost. It takes the place of the one
    /// there is.
    pub fn set_observer(&self, observer: Arc<dyn Observer>) {
        self.shared.observer.set(observer);
    }

    /// Starts a request, due after the deadline the contract states for the method.
    pub fn start_call(&self, method: &str, params: Json) -> Result<Call, CallError> {
        self.start_call_within(method, params, deadline(self.shared.kind, method))
    }

    /// Starts a request that is due after `deadline`.
    pub fn start_call_within(
        &self,
        method: &str,
        params: Json,
        deadline: Duration,
    ) -> Result<Call, CallError> {
        let connection = self.shared.live().connection.clone();
        match connection {
            Some(connection) => connection.send(method, params, deadline),
            None => Err(self.shared.unavailable()),
        }
    }

    /// Calls `method` and waits for its result.
    pub async fn call(&self, method: &str, params: Json) -> Result<Json, CallError> {
        self.start_call(method, params)?.await
    }

    /// Stops the plugin: a program that was started is told to finish, and killed if it has not exited in ten
    /// seconds; a service that was attached to is let go of. Requests that are waiting are failed. Calling it
    /// again does nothing.
    pub async fn shutdown(&self) {
        let _ = self.shared.stop.send(true);
        let task = self
            .shared
            .task
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(task) = task {
            // The task ends by itself once it has seen the signal; a task that was cut short has nothing to add.
            let _ = task.await;
        }
        self.shared.retire();
    }
}

/// How one session with a plugin came to an end.
enum Outcome {
    /// The host stopped it.
    Stopped,
    /// It answered hello and was refused: it is stopped for good.
    Refused(Refusal),
    /// It failed or finished.
    Ended {
        ended: Ended,
        detail: String,
        ran_for: Duration,
    },
}

async fn supervise(shared: Arc<Shared>) {
    let mut stop = shared.stop.subscribe();
    let mut attempt: u32 = 0;
    loop {
        if *stop.borrow() {
            break;
        }
        match session(&shared, &mut stop).await {
            Outcome::Stopped => break,
            Outcome::Refused(refusal) => {
                tracing::error!(
                    kind = %shared.kind, plugin = %shared.name, %refusal,
                    "the plugin was refused and is stopped"
                );
                shared.change(|report| {
                    report.state = PluginState::Failed;
                    report.detail = Some(refusal.to_string());
                    report.settled = true;
                });
                return;
            }
            Outcome::Ended {
                ended,
                detail,
                ran_for,
            } => {
                let again = match &shared.start {
                    PluginStart::Command { restart, .. } => restart.restarts(ended),
                    PluginStart::Attach(_) => true,
                };
                if !again {
                    tracing::warn!(
                        kind = %shared.kind, plugin = %shared.name, %detail,
                        "the plugin is gone and its registration leaves it down"
                    );
                    shared.change(|report| {
                        report.state = PluginState::Failed;
                        report.detail = Some(detail);
                        report.settled = true;
                    });
                    return;
                }
                if ran_for >= BACKOFF_MOST {
                    attempt = 0;
                }
                let pause = backoff(attempt);
                attempt = attempt.saturating_add(1);
                // The moment is fixed before anybody can see that the plugin is restarting.
                let now = shared.options.clock.time();
                let due = now.unix_millis().saturating_add(millis(pause));
                let retry_at = chrono::Duration::from_std(pause)
                    .ok()
                    .and_then(|pause| now.get().checked_add_signed(pause))
                    .map_or(now, Time::new);
                tracing::warn!(
                    kind = %shared.kind, plugin = %shared.name, %detail, pause_secs = pause.as_secs(),
                    "the plugin is gone and will be started again"
                );
                shared.change(|report| {
                    report.state = PluginState::Restarting;
                    report.detail = Some(detail);
                    report.retry_at = Some(retry_at);
                    report.settled = true;
                });
                if !rest(&shared, due, &mut stop).await {
                    break;
                }
            }
        }
    }
    shared.change(|report| {
        report.state = PluginState::Failed;
        report.detail = Some("it was stopped by the host".to_owned());
        report.retry_at = None;
        report.settled = true;
    });
}

fn millis(span: Duration) -> i64 {
    i64::try_from(span.as_millis()).unwrap_or(i64::MAX)
}

/// Resolves when the host has asked for the plugin to be stopped. The guard that `wait_for` returns is not held
/// across anything else, so that the callers' futures stay `Send`.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    // The sender lives as long as the plugin does; if it is gone there is nobody to run for.
    let _ = stop.wait_for(|stopped| *stopped).await;
}

/// Waits until the clock shows `due` (in milliseconds since the epoch). `false` if the host stopped meanwhile.
async fn rest(shared: &Shared, due: i64, stop: &mut watch::Receiver<bool>) -> bool {
    let clock = &shared.options.clock;
    let mut ticker = tokio::time::interval(shared.options.timing.tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if clock.time().unix_millis() >= due {
                    return true;
                }
            }
            () = stopped(stop) => return false,
        }
    }
}

/// Resolves when the program has ended; never for a plugin that was attached to.
async fn exit_of(process: &mut Option<Process>) -> io::Result<ExitStatus> {
    match process {
        Some(process) => process.wait().await,
        None => pending().await,
    }
}

/// What the end of a program says about how it ended.
fn judge(status: &io::Result<ExitStatus>) -> (Ended, String) {
    match status {
        Ok(status) if status.success() => (
            Ended::Clean,
            "the plugin exited by itself with success".to_owned(),
        ),
        Ok(status) => (Ended::Failed, format!("the plugin ended: {status}")),
        Err(error) => (
            Ended::Failed,
            format!("the plugin's end could not be learned: {error}"),
        ),
    }
}

/// Hands a line of the plugin to the connection.
async fn feed(connection: &Connection, line: Line) {
    match line {
        Line::Text(text) => {
            if !text.trim().is_empty() {
                connection.on_line(&text).await;
            }
        }
        Line::Oversized => connection.malformed("a line over the bound"),
        Line::NotUtf8 => connection.malformed("a line that is not text"),
    }
}

/// Reads what is left in the pipe of a plugin that has ended.
async fn drain(reader: &mut LineReader, connection: &Connection) {
    let mut remaining = DRAIN_WITHIN;
    loop {
        let began = tokio::time::Instant::now();
        let line = tokio::time::timeout(remaining, reader.next()).await;
        remaining = remaining.saturating_sub(began.elapsed());
        let Ok(Ok(Some(line))) = line else { break };
        // A decoded frame is retained through bounded host delivery. Its capacity wait is not a
        // native pipe read; the original native read budget and active-consumer deadlines remain.
        feed(connection, line).await;
        if remaining.is_zero() {
            break;
        }
    }
}

/// Writes the lines that are queued, in order, and closes the plugin's input when there are no more.
async fn write_lines(
    mut writer: Box<dyn AsyncWrite + Send + Unpin>,
    mut lines: mpsc::UnboundedReceiver<String>,
) {
    while let Some(line) = lines.recv().await {
        if writer.write_all(line.as_bytes()).await.is_err() || writer.flush().await.is_err() {
            return;
        }
    }
    // The end of the input is how a program learns that nothing more will be said.
    let _ = writer.shutdown().await;
}

/// Why `serve` stopped.
enum Cause {
    Stopped,
    Exited(io::Result<ExitStatus>),
    Disconnected(String),
    Silent,
}

/// Reads the plugin, looks at the clock and watches the program, until one of them says that the session is over.
struct Served {
    cause: Cause,
    pending: Option<futures::future::BoxFuture<'static, ()>>,
}

async fn serve(
    shared: &Shared,
    connection: &Arc<Connection>,
    reader: &mut LineReader,
    process: &mut Option<Process>,
    stop: &mut watch::Receiver<bool>,
) -> Served {
    let mut ticker = tokio::time::interval(shared.options.timing.tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut feeding: Option<futures::future::BoxFuture<'static, ()>> = None;
    let cause = loop {
        tokio::select! {
            read = reader.next(), if feeding.is_none() => match read {
                Ok(Some(line)) => {
                    let connection = Arc::clone(connection);
                    feeding = Some(Box::pin(async move { feed(&connection, line).await }));
                },
                Ok(None) => break Cause::Disconnected("the plugin closed its output".to_owned()),
                Err(error) => break Cause::Disconnected(format!("the plugin could not be read: {error}")),
            },
            () = async { if let Some(future) = feeding.as_mut() { future.await } }, if feeding.is_some() => {
                feeding = None;
            },
            _ = ticker.tick() => {
                if connection.tick() == Verdict::Dead {
                    if feeding.is_some() {
                        break Cause::Disconnected("the host could not consume plugin output before the liveness deadline".into());
                    }
                    break Cause::Silent;
                }
            }
            status = exit_of(process) => break Cause::Exited(status),
            () = stopped(stop) => break Cause::Stopped,
        }
    };
    Served {
        cause,
        pending: feeding,
    }
}

/// One session: open the plugin, negotiate, serve, and take it down.
async fn session(shared: &Arc<Shared>, stop: &mut watch::Receiver<bool>) -> Outcome {
    let clock = &shared.options.clock;
    let opened = tokio::select! {
        opened = transport::open(&shared.start) => opened,
        () = stopped(stop) => return Outcome::Stopped,
    };
    let mut link = match opened {
        Ok(link) => link,
        Err(error) => {
            return Outcome::Ended {
                ended: Ended::Failed,
                detail: error.to_string(),
                ran_for: Duration::ZERO,
            };
        }
    };
    // The hello is waited for from this moment, which is fixed before anybody can see that the plugin started.
    let since = clock.time();
    shared.change(|report| {
        report.starts += 1;
        report.retry_at = None;
    });
    let mut log = link
        .log
        .take()
        .map(|log| transport::forward_log(log, shared.kind, shared.name.clone()));

    let handshake = Handshake {
        plugin: &shared.name,
        kind: shared.kind,
        host_version: &shared.options.host_version,
        clock: clock.as_ref(),
        tick: shared.options.timing.tick,
        since,
        within: hello_deadline(shared.kind),
        counters: &shared.counters,
    };
    let negotiated = tokio::select! {
        negotiated = handshake.run(&mut link) => negotiated,
        () = stopped(stop) => {
            take_down(&mut link.process, log.take()).await;
            return Outcome::Stopped;
        }
    };
    let negotiated = match negotiated {
        Ok(negotiated) => Arc::new(negotiated),
        Err(Failure::Refused(refusal)) => {
            take_down(&mut link.process, log.take()).await;
            return Outcome::Refused(refusal);
        }
        Err(failure @ (Failure::Gone(_) | Failure::Silent(_))) => {
            // A program that closed its output is on its way out, and what it ended with is worth learning; one
            // that said nothing is hung, and is ended at once.
            let exit = match (&failure, link.process.as_mut()) {
                (Failure::Gone(_), Some(process)) => {
                    match tokio::time::timeout(DRAIN_WITHIN, process.wait()).await {
                        Ok(status) => Some(judge(&status).1),
                        Err(_) => None,
                    }
                }
                _ => None,
            };
            take_down(&mut link.process, log.take()).await;
            let detail = failure.to_string();
            return Outcome::Ended {
                ended: Ended::Failed,
                detail: exit.map_or(detail.clone(), |exit| format!("{detail}; {exit}")),
                ran_for: Duration::ZERO,
            };
        }
    };

    let (outgoing, lines) = mpsc::unbounded_channel();
    let writer = tokio::spawn(write_lines(link.writer, lines));
    let connection = Connection::new(
        &shared.name,
        Arc::clone(clock),
        outgoing,
        Arc::clone(&shared.observer),
        Arc::clone(&shared.counters),
    );
    let began = clock.time();
    {
        let mut live = shared.live();
        live.connection = Some(Arc::clone(&connection));
        live.negotiated = Some(Arc::clone(&negotiated));
    }
    tracing::info!(
        kind = %shared.kind, plugin = %shared.name, version = %negotiated.identity.version,
        transport = negotiated.transport, contract = negotiated.contract,
        "the plugin is running"
    );
    shared.change(|report| {
        report.state = PluginState::Running;
        report.version.clone_from(&negotiated.identity.version);
        report.detail = None;
        report.retry_at = None;
        report.versions = Some(Versions {
            transport: negotiated.transport,
            contract: negotiated.contract,
        });
        report.unavailable = negotiated
            .methods
            .unavailable()
            .into_iter()
            .map(str::to_owned)
            .collect();
        report.settled = true;
    });

    let Served { cause, pending } = serve(
        shared,
        &connection,
        &mut link.reader,
        &mut link.process,
        stop,
    )
    .await;
    let (ended, detail) = match cause {
        Cause::Stopped => {
            connection.stop_accepting("the host stopped the plugin");
            if pending.is_none() {
                finish(shared, &connection, &mut link.reader, &mut link.process).await;
            }
            writer.abort();
            take_down(&mut link.process, log.take()).await;
            shared.live().connection = None;
            connection.close_waiters("the host stopped the plugin");
            if let Some(pending) = pending {
                pending.await;
            }
            connection.close("the host stopped the plugin").await;
            return Outcome::Stopped;
        }
        Cause::Exited(status) => {
            connection.stop_accepting("the plugin process ended");
            if pending.is_some() {
                connection
                    .close_waiters("the plugin ended while output delivery was backpressured");
            }
            if let Some(pending) = pending {
                pending.await;
            }
            drain(&mut link.reader, &connection).await;
            judge(&status)
        }
        Cause::Disconnected(why) => {
            if pending.is_some() {
                connection.close_waiters(&why);
                shared.live().connection = None;
                take_down(&mut link.process, log.take()).await;
            }
            if let Some(pending) = pending {
                pending.await;
            }
            match link.process.as_mut() {
                Some(process) => match tokio::time::timeout(DRAIN_WITHIN, process.wait()).await {
                    Ok(status) => {
                        let (ended, how) = judge(&status);
                        (ended, format!("{why}; {how}"))
                    }
                    Err(_) => (Ended::Failed, format!("{why}; it was ended")),
                },
                None => (Ended::Failed, why),
            }
        }
        Cause::Silent => {
            take_down(&mut link.process, log.take()).await;
            if let Some(pending) = pending {
                pending.await;
            }
            (
                Ended::Failed,
                "the plugin did not answer a ping in time and was ended".to_owned(),
            )
        }
    };
    tracing::warn!(kind = %shared.kind, plugin = %shared.name, %detail, "the plugin is gone");
    shared.live().connection = None;
    connection.close_waiters(&detail);
    writer.abort();
    take_down(&mut link.process, log.take()).await;
    connection.close(&detail).await;
    let ran_for = u64::try_from(clock.time().unix_millis() - began.unix_millis()).unwrap_or(0);
    Outcome::Ended {
        ended,
        detail,
        ran_for: Duration::from_millis(ran_for),
    }
}

/// Ends the program if it is still running, and lets the reading of its log finish: what a program wrote just
/// before it died is what an operator needs most.
async fn take_down(process: &mut Option<Process>, log: Option<JoinHandle<()>>) {
    if let Some(process) = process.as_mut() {
        process.kill().await;
    }
    if let Some(mut log) = log {
        // The log ends with the program; a reading that has not ended by now holds on to something else.
        if tokio::time::timeout(DRAIN_WITHIN, &mut log).await.is_err() {
            log.abort();
        }
    }
}

/// The host stops: a program is told to finish, and given ten seconds; a service is let go of.
async fn finish(
    shared: &Shared,
    connection: &Connection,
    reader: &mut LineReader,
    process: &mut Option<Process>,
) {
    connection.stop_accepting("the host is shutting down");
    let Some(process) = process.as_mut() else {
        return;
    };
    // The ten seconds count from before the program can have been told.
    let clock = &shared.options.clock;
    let due = clock
        .time()
        .unix_millis()
        .saturating_add(millis(SHUTDOWN_WITHIN));
    connection.say(&ToPlugin::Shutdown);
    connection.finish_writing();
    let mut ticker = tokio::time::interval(shared.options.timing.tick);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut reading = true;
    loop {
        tokio::select! {
            read = reader.next(), if reading => match read {
                Ok(Some(line)) => feed(connection, line).await,
                Ok(None) | Err(_) => reading = false,
            },
            status = process.wait() => {
                let _ = status;
                drain(reader, connection).await;
                return;
            }
            _ = ticker.tick() => {
                if clock.time().unix_millis() >= due {
                    tracing::warn!(
                        kind = %shared.kind, plugin = %shared.name,
                        "the plugin did not exit after shutdown and is ended"
                    );
                    return;
                }
            }
        }
    }
}
