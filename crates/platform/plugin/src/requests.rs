//! Matching overlapping requests, deadlines, progress and cancellation.
//!
//! One [`Connection`] is one session with a plugin, from its handshake to the moment it is gone. It holds the
//! requests that are waiting for an answer, numbers new ones, matches each answer to its request by id, in
//! whatever order the answers come, and fails what ran out of time. The rules are the contract's own and pure
//! (`orch_plugin_contract::lifecycle`): [`Requests`] numbers and finds the late ones, [`Liveness`] says when to
//! ping and when to give up, and both are told the time by the [`Clock`] the host was given. Nothing here sleeps;
//! the supervisor calls [`Connection::tick`] often enough that a deadline is kept to within a tick.
//!
//! A request is started with [`Connection::send`], which never waits and returns a [`Call`]: the request's id, the
//! means to cancel it, and a future for its answer. A call that is dropped before its answer came tells the plugin
//! to stop working on it, so cancelling is as simple as not waiting any more. Progress of a request, events of the
//! plugin and the loss of the connection go to the [`Observer`], never to the caller of a request: progress does
//! not extend a deadline, and an event belongs to no request.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread;
use std::time::{Duration, Instant};

use orch_plugin_contract::lifecycle::{Liveness, LivenessAction, PING_REPLY_WITHIN, Requests};
use orch_plugin_contract::messages::{
    Event, FromPlugin, PluginError, ProgressParams, Reply, RequestId, ToPlugin, code, method,
};
use orch_types::{Clock, Json};
use tokio::sync::{mpsc, oneshot};

/// How much longer than a request's deadline a thread that blocks for its answer waits before it gives up by
/// itself: five seconds. The deadline is kept by the supervisor's ticks; a thread that waits for longer than the
/// deadline and this much is a thread that blocks the very runtime the ticks run on, and it must not hang for
/// good.
pub const BLOCKING_SLACK: Duration = Duration::from_secs(5);

/// The code of the error for a plugin that cannot be reached: it is down, being restarted, or stopped for good.
pub const UNAVAILABLE: &str = "unavailable";

/// Something that watches a plugin: what it says unasked, how far its requests have come, and when it is gone.
///
/// Its methods preserve input order; awaiting an event applies backpressure to the plugin reader.
#[async_trait::async_trait]
pub trait Observer: Send + Sync + 'static {
    /// The plugin sent an event.
    async fn event(&self, event: Event) {
        let _ = event;
    }

    /// The plugin reported how far request `request`, a call of `method`, has come.
    fn progress(&self, request: RequestId, method: &str, progress: ProgressParams) {
        let _ = (request, method, progress);
    }

    /// The session with the plugin ended, for `reason`. Whatever was started on it is gone.
    async fn lost(&self, reason: &str) {
        let _ = reason;
    }
}

/// The observer a plugin has, shared by its sessions: a plugin that is restarted keeps its observer.
#[derive(Default)]
pub struct ObserverSlot(RwLock<Option<Arc<dyn Observer>>>);

impl ObserverSlot {
    /// Puts `observer` in the place of the one there is.
    pub fn set(&self, observer: Arc<dyn Observer>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(observer);
    }

    fn get(&self) -> Option<Arc<dyn Observer>> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl std::fmt::Debug for ObserverSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ObserverSlot")
    }
}

/// What the transport counted about a plugin over all its sessions.
#[derive(Debug, Default)]
pub struct Counters {
    malformed_lines: AtomicU64,
}

impl Counters {
    /// How many lines of the plugin were skipped because they were no message.
    #[must_use]
    pub fn malformed_lines(&self) -> u64 {
        self.malformed_lines.load(Ordering::Relaxed)
    }

    /// Counts a line of the plugin `plugin` that is no message, and says so once for every doubling of the
    /// count, so that a plugin that writes nothing else does not fill the log. The line itself is never
    /// written: it may hold what the plugin was given in confidence.
    pub fn skipped_line(&self, plugin: &str, why: &str) {
        let count = self.malformed_lines.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_power_of_two() {
            tracing::warn!(
                plugin,
                count,
                why,
                "a line of the plugin is no message and was skipped"
            );
        }
    }
}

/// An error a plugin answered with, as it states it: the transport's three members and the two that a notifier's
/// refusal adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    /// The code.
    pub code: String,
    /// What happened, in a sentence.
    pub message: String,
    /// Whether trying again may help.
    pub retryable: bool,
    /// How long the far side a notifier reaches asked to wait, in seconds.
    pub retry_after_secs: Option<u32>,
    /// Whether the target of a delivery no longer exists.
    pub gone: bool,
}

impl RemoteError {
    /// An error of `code`, for something that failed on this side.
    #[must_use]
    pub fn new(code: &str, message: &str, retryable: bool) -> Self {
        Self {
            code: code.to_owned(),
            message: message.to_owned(),
            retryable,
            retry_after_secs: None,
            gone: false,
        }
    }
}

/// Why a request did not come back with a result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    /// The plugin is down, being started again, or stopped for good: nothing was sent.
    #[error("the plugin {plugin:?} is unavailable: {detail}")]
    Unavailable {
        /// The plugin's name.
        plugin: String,
        /// Why, as far as the host knows.
        detail: String,
    },
    /// The plugin went away while the request was waiting for its answer.
    #[error("the plugin {plugin:?} went away during {method}: {reason}")]
    Lost {
        /// The plugin's name.
        plugin: String,
        /// The request's method.
        method: String,
        /// How it went.
        reason: String,
    },
    /// No answer came within the method's deadline.
    #[error("{method} was not answered in time")]
    Timeout {
        /// The request's method.
        method: String,
    },
    /// The request was cancelled.
    #[error("the request was cancelled")]
    Cancelled,
    /// The plugin answered with an error.
    #[error("{}: {}", .0.code, .0.message)]
    Failed(RemoteError),
    /// The plugin answered with a result that is not the shape the contract states for the method.
    #[error("the answer to {method} is not what it should be: {detail}")]
    Malformed {
        /// The request's method.
        method: String,
        /// What is wrong with it.
        detail: String,
    },
}

impl CallError {
    /// The code of the error, as a plugin would state it.
    #[must_use]
    pub fn code(&self) -> &str {
        match self {
            Self::Unavailable { .. } | Self::Lost { .. } => UNAVAILABLE,
            Self::Timeout { .. } => code::TIMEOUT,
            Self::Cancelled => code::CANCELLED,
            Self::Failed(error) => &error.code,
            Self::Malformed { .. } => code::INTERNAL,
        }
    }

    /// Whether trying again may help: a plugin that is down comes back, a request that was not answered in time
    /// may be, and an error says for itself.
    #[must_use]
    pub fn retryable(&self) -> bool {
        match self {
            Self::Unavailable { .. } | Self::Lost { .. } | Self::Timeout { .. } => true,
            Self::Cancelled | Self::Malformed { .. } => false,
            Self::Failed(error) => error.retryable,
        }
    }
}

/// What a tick found out about the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It is alive, as far as can be told.
    Alive,
    /// It did not answer a ping in time.
    Dead,
    /// The connection is closed already.
    Closed,
}

struct Waiting {
    method: String,
    reply: oneshot::Sender<Result<Json, CallError>>,
}

struct State {
    requests: Requests,
    waiting: BTreeMap<RequestId, Waiting>,
    liveness: Liveness,
    ping: Option<RequestId>,
    outgoing: Option<mpsc::UnboundedSender<String>>,
    accepting: bool,
    closed_because: String,
}

/// One session with a plugin: the requests in flight on it, and the lines to be written to it.
pub struct Connection {
    plugin: String,
    clock: Arc<dyn Clock>,
    state: Mutex<State>,
    observer: Arc<ObserverSlot>,
    counters: Arc<Counters>,
}

/// What is to be told to the observer once the lock on the state is let go.
enum Notice {
    Event(Event),
    Progress(RequestId, String, ProgressParams),
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("plugin", &self.plugin)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// A session with the plugin `plugin` that was heard from just now, whose lines are written to `outgoing`.
    #[must_use]
    pub fn new(
        plugin: &str,
        clock: Arc<dyn Clock>,
        outgoing: mpsc::UnboundedSender<String>,
        observer: Arc<ObserverSlot>,
        counters: Arc<Counters>,
    ) -> Arc<Self> {
        let now = clock.time();
        Arc::new(Self {
            plugin: plugin.to_owned(),
            clock,
            state: Mutex::new(State {
                requests: Requests::new(),
                waiting: BTreeMap::new(),
                liveness: Liveness::new(now),
                ping: None,
                outgoing: Some(outgoing),
                accepting: true,
                closed_because: String::new(),
            }),
            observer,
            counters,
        })
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn unavailable(&self, state: &State) -> CallError {
        CallError::Unavailable {
            plugin: self.plugin.clone(),
            detail: if state.closed_because.is_empty() {
                "it is shutting down".to_owned()
            } else {
                state.closed_because.clone()
            },
        }
    }

    /// Starts a request for `method` that is due after `deadline`. The line is queued for the writer; nothing
    /// waits. The request is cancelled if the returned call is dropped before its answer.
    pub fn send(
        self: &Arc<Self>,
        method: &str,
        params: Json,
        deadline: Duration,
    ) -> Result<Call, CallError> {
        let (reply, answer) = oneshot::channel();
        let id = {
            let mut state = self.lock();
            if !state.accepting || state.outgoing.is_none() {
                return Err(self.unavailable(&state));
            }
            let id = state.requests.open(method, self.clock.time(), deadline);
            state.waiting.insert(
                id,
                Waiting {
                    method: method.to_owned(),
                    reply,
                },
            );
            id
        };
        // The line is made without the lock held: a request can be large.
        let line = line_of(&ToPlugin::request(id, method, params));
        let queued = self
            .lock()
            .outgoing
            .as_ref()
            .is_some_and(|outgoing| outgoing.send(line).is_ok());
        if !queued {
            let mut state = self.lock();
            state.requests.answered(id);
            state.waiting.remove(&id);
            return Err(CallError::Lost {
                plugin: self.plugin.clone(),
                method: method.to_owned(),
                reason: if state.closed_because.is_empty() {
                    "the plugin cannot be written to".to_owned()
                } else {
                    state.closed_because.clone()
                },
            });
        }
        Ok(Call {
            connection: Arc::clone(self),
            id,
            method: method.to_owned(),
            deadline,
            answer,
            done: false,
        })
    }

    /// Reads one line of the plugin and acts on it: an answer ends its request, progress and events go to the
    /// observer. Whatever it is, a line shows that the plugin is alive; one that is no message is skipped and
    /// counted.
    pub async fn on_line(&self, text: &str) {
        let now = self.clock.time();
        let message = match FromPlugin::parse(text) {
            Ok(message) => message,
            Err(error) => {
                self.lock().liveness.heard(now);
                self.malformed(&error.to_string());
                return;
            }
        };
        let mut notices = Vec::new();
        let mut reply_to_send = None;
        {
            let mut state = self.lock();
            state.liveness.heard(now);
            match message {
                FromPlugin::Reply(Reply { id, outcome }) => {
                    if state.ping == Some(id) {
                        state.ping = None;
                        state.requests.answered(id);
                        state.liveness.ping_answered(now);
                    } else if state.requests.answered(id).is_some() {
                        if let Some(waiting) = state.waiting.remove(&id) {
                            let result = match outcome {
                                Ok(result) => Ok(result),
                                Err(error) if error.code == code::CANCELLED => {
                                    Err(CallError::Cancelled)
                                }
                                Err(error) => Err(CallError::Failed(remote_error(error, text))),
                            };
                            reply_to_send = Some((waiting.reply, result));
                        }
                    } else {
                        tracing::debug!(
                            plugin = %self.plugin, id,
                            "an answer for a request that is not waiting was ignored"
                        );
                    }
                }
                FromPlugin::Progress(progress) => {
                    if let Some(waiting) = state.waiting.get(&progress.progress) {
                        notices.push(Notice::Progress(
                            progress.progress,
                            waiting.method.clone(),
                            progress.params,
                        ));
                    }
                }
                FromPlugin::Event(event) => notices.push(Notice::Event(event)),
            }
        }
        if let Some((reply, result)) = reply_to_send {
            // The caller may have stopped waiting; that is its right.
            let _ = reply.send(result);
        }
        if let Some(observer) = self.observer.get() {
            for notice in notices {
                match notice {
                    Notice::Event(event) => observer.event(event).await,
                    Notice::Progress(id, method, progress) => {
                        observer.progress(id, &method, progress);
                    }
                }
            }
        }
    }

    /// Counts a line of the plugin that is no message ([`Counters::skipped_line`]).
    pub fn malformed(&self, why: &str) {
        self.counters.skipped_line(&self.plugin, why);
    }

    /// What the passing of time does: requests that ran out of time fail as `timeout` (and the plugin is told to
    /// stop working on them), a plugin that has been silent is pinged, and one that did not answer a ping is
    /// declared dead.
    #[must_use]
    pub fn tick(&self) -> Verdict {
        let now = self.clock.time();
        let mut late = Vec::new();
        let verdict = {
            let mut state = self.lock();
            if state.outgoing.is_none() {
                return Verdict::Closed;
            }
            for (id, _) in state.requests.expired(now) {
                if state.ping == Some(id) {
                    continue;
                }
                if let Some(waiting) = state.waiting.remove(&id) {
                    late.push((id, waiting));
                }
            }
            for (id, _) in &late {
                queue(&state, line_of(&ToPlugin::Cancel { id: *id }));
            }
            match state.liveness.next(now) {
                LivenessAction::Wait => Verdict::Alive,
                LivenessAction::Ping => {
                    if state.ping.is_none() {
                        let id = state.requests.open(method::PING, now, PING_REPLY_WITHIN);
                        state.ping = Some(id);
                        state.liveness.ping_sent(now);
                        queue(
                            &state,
                            line_of(&ToPlugin::request(id, method::PING, Json::Null)),
                        );
                    }
                    Verdict::Alive
                }
                LivenessAction::Restart => Verdict::Dead,
            }
        };
        for (_, waiting) in late {
            let _ = waiting.reply.send(Err(CallError::Timeout {
                method: waiting.method,
            }));
        }
        verdict
    }

    /// Writes `message` after what is queued, without a request: the way to say `shutdown`.
    pub fn say(&self, message: &ToPlugin) {
        queue(&self.lock(), line_of(message));
    }

    /// Nothing more will be written: the writer finishes what is queued and closes the plugin's input, which is
    /// how a program learns that it will hear nothing more. Requests in flight go on.
    pub fn finish_writing(&self) {
        self.lock().outgoing = None;
    }

    /// No new request is accepted from now on; those in flight go on.
    pub fn stop_accepting(&self, because: &str) {
        let mut state = self.lock();
        state.accepting = false;
        if state.closed_because.is_empty() {
            because.clone_into(&mut state.closed_because);
        }
    }

    /// Ends local calls immediately without waiting for a terminal observer's delivery capacity.
    pub fn close_waiters(&self, reason: &str) {
        let waiting = {
            let mut state = self.lock();
            state.accepting = false;
            state.outgoing = None;
            reason.clone_into(&mut state.closed_because);
            std::mem::take(&mut state.waiting)
        };
        for (_, waiting) in waiting {
            let _ = waiting.reply.send(Err(CallError::Lost {
                plugin: self.plugin.clone(),
                method: waiting.method,
                reason: reason.to_owned(),
            }));
        }
    }

    /// Ends local calls immediately, then delivers the terminal observation with backpressure.
    pub async fn close(&self, reason: &str) {
        self.close_waiters(reason);
        if let Some(observer) = self.observer.get() {
            observer.lost(reason).await;
        }
    }

    /// How many requests are waiting for an answer.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.lock().waiting.len()
    }

    fn cancel(&self, id: RequestId, reply: Option<CallError>) {
        let waiting = {
            let mut state = self.lock();
            if state.requests.answered(id).is_none() {
                return;
            }
            queue(&state, line_of(&ToPlugin::Cancel { id }));
            state.waiting.remove(&id)
        };
        if let (Some(waiting), Some(error)) = (waiting, reply) {
            let _ = waiting.reply.send(Err(error));
        }
    }
}

fn queue(state: &State, line: String) {
    if let Some(outgoing) = &state.outgoing {
        // A writer that has stopped is a plugin that is gone; the session notices that by itself.
        let _ = outgoing.send(line);
    }
}

/// A line as it is written: the message and its end.
fn line_of(message: &ToPlugin) -> String {
    let mut line = message.to_line();
    line.push('\n');
    line
}

/// The error a plugin answered with. The transport's error has three members; a notifier's refusal has two more,
/// which are read from the line itself, since the shared message type does not carry them.
fn remote_error(error: PluginError, text: &str) -> RemoteError {
    let extras = serde_json::from_str::<Json>(text).ok();
    let member = |name: &str| {
        extras
            .as_ref()
            .and_then(|line| line.get("error")?.get(name))
    };
    RemoteError {
        code: error.code,
        message: error.message,
        retryable: error.retryable,
        retry_after_secs: member("retry_after_secs")
            .and_then(Json::as_u64)
            .and_then(|seconds| u32::try_from(seconds).ok()),
        gone: member("gone").and_then(Json::as_bool).unwrap_or(false),
    }
}

/// A request that was started: its id, the means to cancel it, and the future of its answer.
///
/// Awaiting the call gives the result of the request or the reason it has none. A call that is dropped while its
/// answer is still to come cancels the request: the plugin is told to stop working on it, and an answer that
/// still arrives is ignored.
pub struct Call {
    connection: Arc<Connection>,
    id: RequestId,
    method: String,
    deadline: Duration,
    answer: oneshot::Receiver<Result<Json, CallError>>,
    done: bool,
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call")
            .field("id", &self.id)
            .field("method", &self.method)
            .finish_non_exhaustive()
    }
}

impl Call {
    /// The request's id on the wire.
    #[must_use]
    pub fn id(&self) -> RequestId {
        self.id
    }

    /// The request's method.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Tells the plugin to stop working on the request, and ends the wait with [`CallError::Cancelled`] at once;
    /// the plugin's own answer, `cancelled` or a result that was already on its way, is ignored.
    pub fn cancel(&self) {
        self.connection.cancel(self.id, Some(CallError::Cancelled));
    }

    /// Lets the request go on without anybody waiting for it: its answer is read and ignored.
    pub fn detach(mut self) {
        self.done = true;
    }

    /// Waits for the answer on this thread, which stops until it comes. For the traits that are not
    /// asynchronous.
    ///
    /// On a multi-thread runtime the thread is handed back to it while it waits. The thread must not be the only
    /// one that runs the plugin's session. A current-thread runtime is refused immediately with `unsupported`,
    /// before waiting: otherwise no task could read the answer or advance its deadline.
    pub fn wait_blocking(mut self) -> Result<Json, CallError> {
        if tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread
        }) {
            return Err(CallError::Failed(RemoteError::new(
                code::UNSUPPORTED,
                "a synchronous plugin bridge requires a multithread runtime or a calling thread outside the runtime",
                false,
            )));
        }
        let on_multi_thread = tokio::runtime::Handle::try_current().is_ok_and(|handle| {
            handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        });
        let cap = self.deadline.saturating_add(BLOCKING_SLACK);
        let method = self.method.clone();
        let mut wait = move || block_on_for(&mut self, cap);
        let outcome = if on_multi_thread {
            tokio::task::block_in_place(wait)
        } else {
            wait()
        };
        outcome.unwrap_or(Err(CallError::Timeout { method }))
    }
}

impl Future for Call {
    type Output = Result<Json, CallError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.answer).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(answer) => {
                self.done = true;
                Poll::Ready(answer.unwrap_or_else(|_| {
                    Err(CallError::Lost {
                        plugin: self.connection.plugin.clone(),
                        method: self.method.clone(),
                        reason: "the session ended".to_owned(),
                    })
                }))
            }
        }
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        if !self.done {
            self.connection.cancel(self.id, None);
        }
    }
}

/// Wakes the thread that waits.
struct Unpark(thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Polls `future` on this thread until it is ready, or `cap` has passed.
fn block_on_for<F: Future + Unpin>(future: &mut F, cap: Duration) -> Option<F::Output> {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    let started = Instant::now();
    loop {
        if let Poll::Ready(output) = Pin::new(&mut *future).poll(&mut cx) {
            return Some(output);
        }
        thread::park_timeout(cap.checked_sub(started.elapsed())?);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration as Span, TimeZone, Utc};
    use orch_plugin_contract::lifecycle::PING_AFTER_IDLE;
    use orch_types::ManualClock;
    use serde_json::json;

    struct Rig {
        connection: Arc<Connection>,
        written: mpsc::UnboundedReceiver<String>,
        clock: ManualClock,
        counters: Arc<Counters>,
        observed: Arc<Mutex<Vec<String>>>,
    }

    struct Recorder(Arc<Mutex<Vec<String>>>);

    #[async_trait::async_trait]
    impl Observer for Recorder {
        async fn event(&self, event: Event) {
            self.0
                .lock()
                .unwrap()
                .push(format!("event {}", event.event));
        }
        fn progress(&self, request: RequestId, method: &str, progress: ProgressParams) {
            self.0.lock().unwrap().push(format!(
                "progress {request} {method} {}",
                progress.message.unwrap_or_default()
            ));
        }
        async fn lost(&self, reason: &str) {
            self.0.lock().unwrap().push(format!("lost {reason}"));
        }
    }

    fn rig() -> Rig {
        let clock = ManualClock::new(Utc.with_ymd_and_hms(2026, 10, 11, 10, 0, 0).unwrap());
        let (outgoing, written) = mpsc::unbounded_channel();
        let observer = Arc::new(ObserverSlot::default());
        let observed = Arc::new(Mutex::new(Vec::new()));
        observer.set(Arc::new(Recorder(Arc::clone(&observed))));
        let counters = Arc::new(Counters::default());
        let connection = Connection::new(
            "tester",
            Arc::new(clock.clone()),
            outgoing,
            observer,
            Arc::clone(&counters),
        );
        Rig {
            connection,
            written,
            clock,
            counters,
            observed,
        }
    }

    fn secs(seconds: i64) -> Span {
        Span::seconds(seconds)
    }

    fn written(rig: &mut Rig) -> Vec<Json> {
        let mut lines = Vec::new();
        while let Ok(line) = rig.written.try_recv() {
            lines.push(serde_json::from_str(line.trim_end()).unwrap());
        }
        lines
    }

    #[tokio::test]
    async fn two_requests_overlap_and_each_gets_its_own_answer_whatever_the_order() {
        let mut rig = rig();
        let first = rig
            .connection
            .send("create", json!({"n": 1}), Duration::from_secs(60))
            .unwrap();
        let second = rig
            .connection
            .send("list", Json::Null, Duration::from_secs(60))
            .unwrap();
        assert_eq!((first.id(), second.id()), (1, 2), "the handshake was 0");
        assert_eq!(rig.connection.in_flight(), 2);
        let lines = written(&mut rig);
        assert_eq!(
            lines[0],
            json!({"id": 1, "method": "create", "params": {"n": 1}})
        );
        assert_eq!(lines[1], json!({"id": 2, "method": "list"}));
        futures::executor::block_on(rig.connection.on_line(r#"{"id":2,"result":{"units":[]}}"#));
        futures::executor::block_on(rig.connection.on_line(r#"{"id":1,"result":{"unit":"u1"}}"#));
        assert_eq!(second.await.unwrap(), json!({"units": []}));
        assert_eq!(first.await.unwrap(), json!({"unit": "u1"}));
        assert_eq!(rig.connection.in_flight(), 0);
    }

    #[tokio::test]
    async fn an_error_answer_is_the_plugins_own_error_and_cancelled_is_cancelled() {
        let rig = rig();
        let refused = rig
            .connection
            .send("create", Json::Null, Duration::from_secs(60))
            .unwrap();
        let stopped = rig
            .connection
            .send("start", Json::Null, Duration::from_secs(60))
            .unwrap();
        futures::executor::block_on(rig.connection.on_line(
            r#"{"id":1,"error":{"code":"rate_limited","message":"slow","retryable":true,"retry_after_secs":30,"gone":true}}"#,
        ));
        futures::executor::block_on(
            rig.connection
                .on_line(r#"{"id":2,"error":{"code":"cancelled","message":"cancelled"}}"#),
        );
        let Err(CallError::Failed(error)) = refused.await else {
            panic!("an error");
        };
        assert_eq!(
            (
                error.code.as_str(),
                error.retryable,
                error.retry_after_secs,
                error.gone
            ),
            ("rate_limited", true, Some(30), true)
        );
        assert_eq!(stopped.await, Err(CallError::Cancelled));
    }

    #[tokio::test]
    async fn a_request_that_is_not_answered_by_its_deadline_times_out_and_the_plugin_is_told_to_stop()
     {
        let mut rig = rig();
        let call = rig
            .connection
            .send("create", Json::Null, Duration::from_secs(60))
            .unwrap();
        written(&mut rig);
        rig.clock.advance(secs(59));
        assert_eq!(rig.connection.tick(), Verdict::Alive);
        // Progress is not an answer and does not move the deadline.
        futures::executor::block_on(
            rig.connection
                .on_line(r#"{"progress":1,"params":{"message":"pulling","done":1,"total":9}}"#),
        );
        rig.clock.advance(secs(1));
        assert_eq!(rig.connection.tick(), Verdict::Alive);
        assert_eq!(
            call.await,
            Err(CallError::Timeout {
                method: "create".into()
            })
        );
        assert!(written(&mut rig).contains(&json!({"method": "cancel", "params": {"id": 1}})));
        assert_eq!(
            rig.observed.lock().unwrap().as_slice(),
            ["progress 1 create pulling"]
        );
        // A late answer is ignored.
        futures::executor::block_on(rig.connection.on_line(r#"{"id":1,"result":{}}"#));
        assert_eq!(rig.connection.in_flight(), 0);
    }

    #[tokio::test]
    async fn a_call_that_is_dropped_cancels_its_request_and_one_that_is_cancelled_ends_at_once() {
        let mut rig = rig();
        let dropped = rig
            .connection
            .send("turn.start", Json::Null, Duration::from_secs(60))
            .unwrap();
        let mut cancelled = rig
            .connection
            .send("turn.start", Json::Null, Duration::from_secs(60))
            .unwrap();
        written(&mut rig);
        drop(dropped);
        cancelled.cancel();
        assert_eq!((&mut cancelled).await, Err(CallError::Cancelled));
        let lines = written(&mut rig);
        assert!(lines.contains(&json!({"method": "cancel", "params": {"id": 1}})));
        assert!(lines.contains(&json!({"method": "cancel", "params": {"id": 2}})));
        assert_eq!(rig.connection.in_flight(), 0);
        let finished = rig
            .connection
            .send("list", Json::Null, Duration::from_secs(60))
            .unwrap();
        futures::executor::block_on(rig.connection.on_line(r#"{"id":3,"result":{}}"#));
        finished.await.unwrap();
        assert!(
            written(&mut rig)
                .iter()
                .all(|line| line["method"] != "cancel"),
            "a call that has its answer cancels nothing"
        );
    }

    #[tokio::test]
    async fn a_plugin_that_is_silent_is_pinged_and_one_that_does_not_answer_is_dead() {
        let mut rig = rig();
        rig.clock
            .advance(Span::seconds(PING_AFTER_IDLE.as_secs() as i64));
        assert_eq!(rig.connection.tick(), Verdict::Alive);
        assert_eq!(written(&mut rig), [json!({"id": 1, "method": "ping"})]);
        rig.clock.advance(secs(9));
        assert_eq!(rig.connection.tick(), Verdict::Alive, "one ping at a time");
        assert!(written(&mut rig).is_empty());
        rig.clock.advance(secs(1));
        assert_eq!(rig.connection.tick(), Verdict::Dead);
    }

    #[tokio::test]
    async fn the_answer_to_a_ping_and_any_other_line_show_that_the_plugin_is_alive() {
        let mut rig = rig();
        rig.clock.advance(secs(30));
        assert_eq!(rig.connection.tick(), Verdict::Alive);
        written(&mut rig);
        rig.clock.advance(secs(5));
        futures::executor::block_on(rig.connection.on_line(r#"{"id":1,"result":{}}"#));
        rig.clock.advance(secs(29));
        assert_eq!(
            rig.connection.tick(),
            Verdict::Alive,
            "the silence starts over"
        );
        assert!(written(&mut rig).is_empty());
        futures::executor::block_on(
            rig.connection
                .on_line(r#"{"event":"harness.status","params":{}}"#),
        );
        rig.clock.advance(secs(29));
        assert_eq!(rig.connection.tick(), Verdict::Alive, "events are traffic");
        assert_eq!(
            rig.observed.lock().unwrap().as_slice(),
            ["event harness.status"]
        );
    }

    #[tokio::test]
    async fn lines_that_are_no_message_are_skipped_and_counted_and_the_next_one_is_read() {
        let rig = rig();
        let call = rig
            .connection
            .send("list", Json::Null, Duration::from_secs(60))
            .unwrap();
        for bad in [
            "not json",
            "[1,2]",
            r#"{"hello":1}"#,
            r#"{"id":"one","result":{}}"#,
        ] {
            futures::executor::block_on(rig.connection.on_line(bad));
        }
        rig.connection.malformed("a line longer than the bound");
        assert_eq!(rig.counters.malformed_lines(), 5);
        futures::executor::block_on(
            rig.connection
                .on_line(r#"{"id":1,"result":{"units":[]},"served_by":"p1"}"#),
        );
        assert_eq!(call.await.unwrap(), json!({"units": []}));
    }

    #[tokio::test]
    async fn when_the_session_ends_every_waiting_request_fails_and_nothing_new_is_taken() {
        let rig = rig();
        let call = rig
            .connection
            .send("create", Json::Null, Duration::from_secs(60))
            .unwrap();
        futures::executor::block_on(rig.connection.close("the plugin exited with status 3"));
        assert_eq!(
            call.await,
            Err(CallError::Lost {
                plugin: "tester".into(),
                method: "create".into(),
                reason: "the plugin exited with status 3".into()
            })
        );
        let refused = rig
            .connection
            .send("list", Json::Null, Duration::from_secs(60))
            .unwrap_err();
        assert_eq!(refused.code(), UNAVAILABLE);
        assert!(refused.retryable());
        assert_eq!(rig.connection.tick(), Verdict::Closed);
        assert_eq!(
            rig.observed.lock().unwrap().as_slice(),
            ["lost the plugin exited with status 3"]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_thread_that_cannot_be_asynchronous_waits_for_the_answer() {
        let rig = rig();
        let connection = Arc::clone(&rig.connection);
        let call = connection
            .send("translate_request", Json::Null, Duration::from_secs(60))
            .unwrap();
        let answering = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            futures::executor::block_on(connection.on_line(r#"{"id":1,"result":{"ok":true}}"#));
        });
        assert_eq!(call.wait_blocking().unwrap(), json!({"ok": true}));
        answering.await.unwrap();
    }

    #[tokio::test]
    async fn a_current_thread_runtime_is_refused_without_waiting_for_a_plugin_deadline() {
        let rig = rig();
        let call = rig
            .connection
            .send("translate_request", Json::Null, Duration::from_secs(60))
            .unwrap();
        let started = std::time::Instant::now();
        let error = call.wait_blocking().unwrap_err();
        assert_eq!(error.code(), code::UNSUPPORTED);
        assert!(!error.retryable());
        assert!(error.to_string().contains("multithread runtime"));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "the caller is refused promptly, rather than hanging for a method timeout"
        );
    }
}
