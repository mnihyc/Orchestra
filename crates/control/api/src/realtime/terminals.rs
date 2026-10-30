//! Terminal sockets: one screen at a time for a shell that runs on without one, and the replay it is taken up with.
//!
//! A shell and the agent's own terminal each have a socket of their own, apart from the one every tab holds,
//! because they carry raw bytes at interactive latency. It is entered with a ticket: a credential the API
//! gave to one person for one shell, good for thirty seconds and for one use, because a browser cannot set a
//! header on a socket. Tickets are kept in the memory of the process that gave them; a terminal's tunnel ends
//! at the process that holds the machine's gateway, and that is the process that serves the socket.
//!
//! **What a socket does.** It opens a tunnel to the shell through the machine's gateway, reads what the
//! machine kept of the shell's last output (which the machine sends first, without saying how much, so a
//! short settling window decides what was kept and what is new), tells the browser with `ready`, and then
//! carries bytes both ways. What is typed goes to the shell at once and is never drawn by the socket. What
//! the shell prints is read from the tunnel only while the browser is less than a mebibyte behind, so a slow
//! line makes a slow shell and loses nothing.
//!
//! **One screen at a time.** The registry of live attachments gives each shell one place. A second ticket
//! used on the same shell takes it: the first gets `closed` with `attached_elsewhere` and the shell itself
//! is untouched. A tunnel that its machine ended because another process took the shell is told the same.
//!
//! **What the sockets are told from outside.** The routes and the keeper say to a socket that its shell was
//! closed, and why, before they end the shell, so that the socket reports the reason it was given and not
//! the first thing it sees of the shell going away. The keeper is the task that sweeps the shells nobody
//! came back to, the shells of an environment that ended, and, on every move of the authorization epoch,
//! the shells whose opener lost what they were opened by.

use crate::middleware::authentication::{new_token, token_hash};
use crate::realtime::Inner;
use crate::registry::Ctx;
use crate::state::App;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use bytes::{Buf, Bytes};
use chrono::{DateTime, Utc};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use orch_api_contract::terminals::{
    TERMINAL_SUBPROTOCOL, TERMINAL_TICKET_SECONDS, TerminalAttachQuery, TerminalTicket,
};
use orch_db::queries::terminals as rows;
use orch_engine::shells::{Ending, Holding};
use orch_link::tunnels::channel::{OpenError, Tunnel, TunnelEvent};
use orch_link::tunnels::terminal::TerminalHub;
use orch_link_contract::tunnels::{OpenErrorCode, PTY_REPLAY_HEADER_BYTES, Service};
use orch_realtime_contract::terminal::{
    Backpressure, Flow, REPLAY_BYTES, TerminalClientFrame, TerminalClosedFrame,
    TerminalClosedReason, TerminalExitFrame, TerminalPongFrame, TerminalReadyFrame,
    TerminalServerFrame,
};
use orch_realtime_contract::version::CloseCode;
use orch_types::{Id, Time};
use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

/// How many bytes one read of the tunnel asks for: a screenful many times over, and what the machine
/// itself reads from a shell at a time, so that the first of a burst reaches the browser at once.
const READ_BYTES: usize = 16 * 1024;
/// The bounded initial transfer has the same thirty-second budget as opening the native PTY.
/// Its declared length, rather than a quiet period in live output, determines where replay ends.
const REPLAY_WAIT_MAX: Duration = Duration::from_secs(30);
/// How long after the end of a shell's process the socket goes on reading what it printed last, counted
/// from the last byte that came: the bytes ahead of the end are already in the tunnel, and a line that
/// moves nothing for this long has nothing more.
const EXIT_DRAIN: Duration = Duration::from_secs(2);
/// How often the bytes that went through a socket are written to the shell's row, and the environment
/// is told that it was used: often enough that the row is near the truth, rare enough that a busy shell
/// costs the database little.
const TOUCH_EVERY: Duration = Duration::from_secs(5);
/// The most bytes of what a person typed that one frame may carry. A paste is cut into pieces by the
/// browser; this is far above a piece, and below anything that could hold a socket's memory.
const MAX_TYPED_FRAME_BYTES: usize = 256 * 1024;
/// How many frames of the browser wait for the tunnel before the socket stops reading the browser,
/// which makes the browser's own line wait: a bound on what a socket holds for a shell that takes
/// nothing.
const INBOX_FRAMES: usize = 32;
/// How many chunks of what was typed are held for the tunnel at once.
const TYPED_QUEUE: usize = 8;
/// The size of a screen in either direction that a shell is resized to at most: what the machine takes.
const SIZE_MAX: u16 = 1_000;
/// The size an agent's terminal is said to have when its watcher has not said one: the harnesses that
/// are driven through a terminal draw for a wide screen.
const AGENT_SIZE: (u16, u16) = (120, 32);
/// The text the machine fails a tunnel with when another screen took the shell.
const TAKEN_BY_ANOTHER: &str = "another screen took this shell";
/// How often the keeper looks at the database for what has to end: the engine's own look at its work
/// is as often.
const KEEP_EVERY: Duration = Duration::from_secs(1);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------- tickets and the registry

/// What a ticket opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    /// A person's shell.
    Shell(Id),
    /// The agent's own terminal of a thread; typing is let through only with `control`.
    Agent {
        /// The thread.
        thread: Id,
        /// Whether what is typed is passed on.
        control: bool,
    },
}

/// A ticket as it is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    /// The person it was given to.
    pub user: Id,
    /// What it opens.
    pub door: Door,
    expires: DateTime<Utc>,
}

/// What a live attachment is told from outside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Told {
    /// Another screen took the shell.
    Taken,
    /// The shell ended, or its person lost it, for this reason; the socket says so and closes.
    Over(TerminalClosedReason),
    /// The server is stopping: the shell is untouched.
    GoingAway,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Key {
    Shell(Id),
    Agent(Id),
}

struct Live {
    number: u64,
    told: mpsc::UnboundedSender<Told>,
}

/// The tickets given and the attachments held by one API process.
#[derive(Default)]
pub struct Sockets {
    tickets: Mutex<HashMap<Vec<u8>, Ticket>>,
    live: Mutex<HashMap<Key, Live>>,
    hub: Mutex<Option<TerminalHub>>,
    numbers: AtomicU64,
}

impl std::fmt::Debug for Sockets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sockets").finish_non_exhaustive()
    }
}

impl Sockets {
    /// A ticket for `user` to open `door`, good for [`TERMINAL_TICKET_SECONDS`] from `now` and for one use.
    pub fn issue(&self, now: DateTime<Utc>, user: Id, door: Door) -> TerminalTicket {
        let token = new_token("tt_");
        let expires = now + chrono::Duration::seconds(i64::from(TERMINAL_TICKET_SECONDS));
        let mut tickets = lock(&self.tickets);
        // Tickets nobody used do not pile up.
        tickets.retain(|_, held| held.expires > now);
        tickets.insert(
            token_hash(&token),
            Ticket {
                user,
                door,
                expires,
            },
        );
        TerminalTicket {
            ticket: token,
            ticket_expires_at: Time::new(expires),
        }
    }

    /// Takes the ticket `token` out, if it is one that was given and has not run out: a ticket is
    /// good once.
    pub fn redeem(&self, now: DateTime<Utc>, token: &str) -> Option<Ticket> {
        lock(&self.tickets)
            .remove(&token_hash(token))
            .filter(|held| held.expires > now)
    }

    /// Makes this process reach machines through `hub` and not through its link: for a test that has a
    /// machine behind a hub of its own.
    pub fn through(&self, hub: TerminalHub) {
        *lock(&self.hub) = Some(hub);
    }

    fn hub(&self, app: &App) -> TerminalHub {
        lock(&self.hub)
            .clone()
            .unwrap_or_else(|| app.link.terminals().clone())
    }

    /// Whether a screen has the shell `id` now, on this process.
    #[must_use]
    pub fn is_attached(&self, id: Id) -> bool {
        lock(&self.live).contains_key(&Key::Shell(id))
    }

    /// Says to the screen that has the shell `id` or the agent terminal of thread `id`, if one is on
    /// this process, what became of it.
    pub fn tell(&self, id: Id, told: Told) {
        let held: Vec<mpsc::UnboundedSender<Told>> = {
            let live = lock(&self.live);
            [Key::Shell(id), Key::Agent(id)]
                .iter()
                .filter_map(|key| live.get(key).map(|each| each.told.clone()))
                .collect()
        };
        for each in held {
            let _ = each.send(told);
        }
    }

    /// Says `told` to every screen on this process.
    fn tell_all(&self, told: Told) {
        for each in lock(&self.live).values() {
            let _ = each.told.send(told);
        }
    }

    /// Takes the place of `key`, and tells the screen that had it that it was taken.
    fn enter(&self, key: Key) -> (u64, mpsc::UnboundedReceiver<Told>) {
        let number = self.numbers.fetch_add(1, Ordering::Relaxed) + 1;
        let (told, hears) = mpsc::unbounded_channel();
        let before = lock(&self.live).insert(key, Live { number, told });
        if let Some(before) = before {
            let _ = before.told.send(Told::Taken);
        }
        (number, hears)
    }

    /// Gives the place up. True where it was still this attachment's: where another screen took it
    /// meanwhile, the shell is that screen's and is not detached.
    fn leave(&self, key: Key, number: u64) -> bool {
        let mut live = lock(&self.live);
        if live.get(&key).is_some_and(|each| each.number == number) {
            live.remove(&key);
            true
        } else {
            false
        }
    }
}

/// The sockets of this process.
fn sockets(app: &App) -> &Sockets {
    &app.realtime.inner.terminals
}

/// Tells every screen on this process that the server is stopping.
pub(crate) fn stop(inner: &Inner) {
    inner.terminals.tell_all(Told::GoingAway);
}

// ---------------------------------------------------------------- the browser's side of a socket

/// What the writer task sends the browser.
enum Out {
    Data(Bytes),
    Text(String),
    Ping,
    Close(u16, String),
}

/// How far the browser is behind: the bytes read from the tunnel and not yet handed to the socket.
struct Behind {
    bytes: AtomicI64,
    flow: Mutex<Backpressure>,
    resumed: Notify,
}

impl Behind {
    fn new() -> Self {
        Self {
            bytes: AtomicI64::new(0),
            flow: Mutex::new(Backpressure::new()),
            resumed: Notify::new(),
        }
    }

    fn added(&self, count: usize) {
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        let now = self.bytes.fetch_add(count, Ordering::SeqCst) + count;
        let _ = lock(&self.flow).update(now);
    }

    fn handed_over(&self, count: usize) {
        let count = i64::try_from(count).unwrap_or(i64::MAX);
        let now = self.bytes.fetch_sub(count, Ordering::SeqCst) - count;
        if lock(&self.flow).update(now) == Some(Flow::Resume) {
            self.resumed.notify_one();
        }
    }

    fn paused(&self) -> bool {
        lock(&self.flow).paused()
    }
}

/// Writes what the pump hands over to the browser, in order, and says how far behind the browser is.
async fn write(
    mut sink: SplitSink<WebSocket, Message>,
    mut out: mpsc::UnboundedReceiver<Out>,
    behind: Arc<Behind>,
) {
    while let Some(item) = out.recv().await {
        let sent = match item {
            Out::Data(bytes) => {
                let count = bytes.len();
                let sent = sink.send(Message::Binary(bytes)).await;
                behind.handed_over(count);
                sent
            }
            Out::Text(text) => sink.send(Message::Text(text.into())).await,
            Out::Ping => sink.send(Message::Ping(Bytes::new())).await,
            Out::Close(code, reason) => {
                let _ = sink
                    .send(Message::Close(Some(CloseFrame {
                        code,
                        reason: reason.into(),
                    })))
                    .await;
                break;
            }
        };
        if sent.is_err() {
            break;
        }
    }
    let _ = sink.close().await;
}

/// What the browser sent, as the pump needs it.
enum In {
    Typed(Bytes),
    Resize(u16, u16),
    Gone,
    Fault(u16, &'static str),
}

/// When the browser last said anything, in the monotonic time of this process.
struct Heard(Mutex<Instant>);

impl Heard {
    fn now() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Instant::now())))
    }

    fn mark(&self) {
        *lock(&self.0) = Instant::now();
    }

    fn silent_for(&self) -> Duration {
        lock(&self.0).elapsed()
    }
}

/// Reads what the browser sends and hands it to the pump, a frame at a time: the pump takes a frame only
/// when it can use it, which makes the browser's line wait when the shell is slow. A `ping` is answered
/// here and then, ahead of what a slow shell has not taken: a browser that asks whether the line is alive
/// does not wait for the shell to be.
async fn read(
    mut stream: SplitStream<WebSocket>,
    inbox: mpsc::Sender<In>,
    heard: Arc<Heard>,
    out: mpsc::UnboundedSender<Out>,
) {
    loop {
        let item = match stream.next().await {
            None | Some(Ok(Message::Close(_))) | Some(Err(_)) => In::Gone,
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {
                heard.mark();
                continue;
            }
            Some(Ok(Message::Binary(bytes))) => {
                heard.mark();
                if bytes.len() > MAX_TYPED_FRAME_BYTES {
                    In::Fault(CloseCode::Malformed.code(), "that frame is too large")
                } else {
                    In::Typed(bytes)
                }
            }
            Some(Ok(Message::Text(text))) => {
                heard.mark();
                match TerminalClientFrame::decode(text.as_str()) {
                    Ok(TerminalClientFrame::Resize(size)) => In::Resize(size.cols, size.rows),
                    Ok(TerminalClientFrame::Ping(ping)) => {
                        let pong = TerminalServerFrame::Pong(TerminalPongFrame { n: ping.n });
                        let _ = out.send(frame(&pong));
                        continue;
                    }
                    Err(_) => In::Fault(
                        CloseCode::Malformed.code(),
                        "that is not a frame of the protocol",
                    ),
                }
            }
        };
        let last = matches!(item, In::Gone | In::Fault(..));
        if inbox.send(item).await.is_err() || last {
            return;
        }
    }
}

// ---------------------------------------------------------------- entering a socket

/// What a socket is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Shell(Id),
    Agent(Id),
}

impl Target {
    fn key(self) -> Key {
        match self {
            Self::Shell(id) => Key::Shell(id),
            Self::Agent(id) => Key::Agent(id),
        }
    }

    /// Whether `door` is the one that opens this.
    fn opened_by(self, door: Door) -> Option<bool> {
        match (self, door) {
            (Self::Shell(id), Door::Shell(held)) if id == held => Some(false),
            (Self::Agent(id), Door::Agent { thread, control }) if id == thread => Some(control),
            _ => None,
        }
    }
}

/// The subprotocols a client offered, in the order it offered them.
fn offered(ctx: &Ctx) -> Vec<String> {
    ctx.headers
        .get_all(http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|each| each.trim().to_owned())
        .filter(|each| !each.is_empty())
        .collect()
}

/// `GET /ws/terminals/{terminal_id}`: a person's shell.
pub async fn shell_socket(
    ctx: Ctx,
    query: TerminalAttachQuery,
    upgrade: WebSocketUpgrade,
) -> Response {
    match ctx.id("terminal_id") {
        Ok(id) => enter(ctx, query, upgrade, Target::Shell(id)),
        Err(error) => error.into_response_for(&ctx.request_id),
    }
}

/// `GET /ws/agent-terminals/{thread_id}`: the agent's own terminal of a thread.
pub async fn agent_socket(
    ctx: Ctx,
    query: TerminalAttachQuery,
    upgrade: WebSocketUpgrade,
) -> Response {
    match ctx.id("thread_id") {
        Ok(id) => enter(ctx, query, upgrade, Target::Agent(id)),
        Err(error) => error.into_response_for(&ctx.request_id),
    }
}

/// Reads the ticket, and upgrades. A client that offers no protocol of ours, and one whose ticket is not
/// good, are let in and closed at once with a code: a browser sees the code of a close and not the
/// refusal of a handshake.
fn enter(
    ctx: Ctx,
    query: TerminalAttachQuery,
    upgrade: WebSocketUpgrade,
    target: Target,
) -> Response {
    let offers = offered(&ctx);
    let speaks = offers.iter().any(|each| each == TERMINAL_SUBPROTOCOL);
    let chosen = if speaks {
        vec![TERMINAL_SUBPROTOCOL.to_owned()]
    } else {
        offers.first().cloned().into_iter().collect()
    };
    let app = ctx.app.clone();
    let origin = crate::sign_in::origin(&ctx);
    // The ticket is taken now, so that the same address used twice is used once.
    let ticket = sockets(&app).redeem(app.clock.now(), &query.ticket);
    upgrade
        .protocols(chosen)
        .max_message_size(MAX_TYPED_FRAME_BYTES)
        .max_frame_size(MAX_TYPED_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            let (sink, stream) = socket.split();
            if !speaks {
                refuse(
                    sink,
                    CloseCode::Unsupported,
                    "that protocol is not spoken here",
                )
                .await;
                return;
            }
            let Some(ticket) = ticket else {
                refuse(sink, CloseCode::Unauthenticated, "the ticket is not good").await;
                return;
            };
            let Some(control) = target.opened_by(ticket.door) else {
                refuse(
                    sink,
                    CloseCode::Unauthenticated,
                    "the ticket is for another terminal",
                )
                .await;
                return;
            };
            let who = Who {
                user: ticket.user,
                control,
            };
            let attachment = Attachment {
                app: app.clone(),
                target,
                who,
                origin,
            };
            attachment.serve(sink, stream).await;
        })
}

async fn refuse(mut sink: SplitSink<WebSocket, Message>, code: CloseCode, reason: &str) {
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: code.code(),
            reason: reason.to_owned().into(),
        })))
        .await;
    let _ = sink.close().await;
}

#[derive(Debug, Clone, Copy)]
struct Who {
    user: Id,
    control: bool,
}

/// A socket that has been let in.
struct Attachment {
    app: App,
    target: Target,
    who: Who,
    origin: orch_db::audit::Origin,
}

/// How an attachment ended.
enum Ended {
    /// The shell's process ended with a code.
    Exit(i64),
    /// Another screen took the shell.
    Taken,
    /// The shell was closed from outside, for this reason.
    Over(TerminalClosedReason),
    /// The line to the machine broke; the shell may run on.
    LinkLost,
    /// The browser went away; the shell runs on.
    BrowserGone,
    /// The browser sent something that is not the protocol.
    Fault(u16, &'static str),
    /// The server is stopping.
    GoingAway,
}

fn frame(frame: &TerminalServerFrame) -> Out {
    Out::Text(serde_json::to_string(frame).unwrap_or_default())
}

fn closed(reason: TerminalClosedReason) -> Out {
    frame(&TerminalServerFrame::Closed(TerminalClosedFrame { reason }))
}

/// The socket's close code for an orderly end: the frame before it said why.
const NORMAL: u16 = 1000;

impl Attachment {
    fn inner(&self) -> &Arc<Inner> {
        &self.app.realtime.inner
    }

    async fn serve(self, sink: SplitSink<WebSocket, Message>, stream: SplitStream<WebSocket>) {
        let behind = Arc::new(Behind::new());
        let (out, outgoing) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write(sink, outgoing, Arc::clone(&behind)));
        self.run(stream, &out, &behind).await;
        // Whatever the writer was handed is given a moment to reach the browser; a browser that takes
        // nothing is not waited for, and its socket is let go.
        drop(out);
        let mut writer = writer;
        if tokio::time::timeout(self.inner().tuning.close_grace, &mut writer)
            .await
            .is_err()
        {
            writer.abort();
        }
    }

    /// The whole life of the attachment, up to the frames that end it.
    async fn run(
        &self,
        stream: SplitStream<WebSocket>,
        out: &mpsc::UnboundedSender<Out>,
        behind: &Arc<Behind>,
    ) {
        let goodbye = |reason: TerminalClosedReason| {
            let _ = out.send(closed(reason));
            let _ = out.send(Out::Close(NORMAL, String::new()));
        };
        let prepared = match self.target {
            Target::Shell(id) => self.prepare_shell(id).await,
            Target::Agent(thread) => self.prepare_agent(thread).await,
        };
        let ready = match prepared {
            Ok(ready) => ready,
            Err(reason) => {
                goodbye(reason);
                return;
            }
        };
        let key = self.target.key();
        let (number, told) = self.inner().terminals.enter(key);
        let opened = self.open_tunnel(&ready).await;
        let mut tunnel = match opened {
            Ok(tunnel) => tunnel,
            Err(reason) => {
                self.inner().terminals.leave(key, number);
                goodbye(reason);
                return;
            }
        };
        let events = tunnel.take_events();
        let replay = match self.replay(&mut tunnel).await {
            Ok(replay) => replay,
            Err(_) => {
                self.inner().terminals.leave(key, number);
                goodbye(TerminalClosedReason::LinkLost);
                return;
            }
        };
        let (cols, rows_) = ready.size;
        let announced = TerminalServerFrame::Ready(TerminalReadyFrame {
            cols,
            rows: rows_,
            replay_bytes: i64::try_from(replay.len()).unwrap_or(i64::MAX),
        });
        let _ = out.send(frame(&announced));
        if !replay.is_empty() {
            let _ = out.send(Out::Data(Bytes::from(replay)));
        }
        if let Target::Shell(id) = self.target {
            let _ = self.app.engine.shells().attached(id).await;
        }
        let heard = Heard::now();
        let (inbox_tx, inbox) = mpsc::channel(INBOX_FRAMES);
        let reader = tokio::spawn(read(stream, inbox_tx, Arc::clone(&heard), out.clone()));
        let mut pump = Pump {
            attachment: self,
            tunnel,
            events,
            inbox,
            told,
            out,
            behind: Arc::clone(behind),
            heard,
            typed: VecDeque::new(),
            exit: None,
            counted: (0, 0),
        };
        let ended = pump.run().await;
        reader.abort();
        let current = self.inner().terminals.leave(key, number);
        self.conclude(ended, current, pump.counted, out).await;
    }

    /// Says how an attachment ended to the browser, and to the row of the shell.
    async fn conclude(
        &self,
        ended: Ended,
        current: bool,
        counted: (i64, i64),
        out: &mpsc::UnboundedSender<Out>,
    ) {
        let shells = self.app.engine.shells();
        let shell = match self.target {
            Target::Shell(id) => Some(id),
            Target::Agent(_) => None,
        };
        // What went through is written first, so that the end that follows is the last word of the row.
        if let Some(id) = shell
            && counted != (0, 0)
        {
            let _ = shells.used(id, counted.0, counted.1).await;
        }
        let last = |reason: TerminalClosedReason| {
            let _ = out.send(closed(reason));
            let _ = out.send(Out::Close(NORMAL, String::new()));
        };
        match ended {
            Ended::Exit(code) => {
                let _ = out.send(frame(&TerminalServerFrame::Exit(TerminalExitFrame {
                    code,
                })));
                if let Some(id) = shell
                    && let Err(error) = shells
                        .finish(id, Ending::exited(code), &orch_db::audit::Origin::default())
                        .await
                {
                    tracing::warn!(%error, "the end of a shell could not be written");
                }
                last(TerminalClosedReason::Exited);
            }
            Ended::Taken => last(TerminalClosedReason::AttachedElsewhere),
            Ended::Over(reason) => last(reason),
            Ended::LinkLost => {
                if current && let Some(id) = shell {
                    let _ = shells.detached(id).await;
                }
                last(TerminalClosedReason::LinkLost);
            }
            Ended::BrowserGone => {
                if current && let Some(id) = shell {
                    let _ = shells.detached(id).await;
                }
            }
            Ended::Fault(code, reason) => {
                if current && let Some(id) = shell {
                    let _ = shells.detached(id).await;
                }
                let _ = out.send(Out::Close(code, reason.to_owned()));
            }
            Ended::GoingAway => {
                if current && let Some(id) = shell {
                    let _ = shells.detached(id).await;
                }
                let _ = out.send(Out::Close(
                    CloseCode::GoingAway.code(),
                    "the server is stopping".to_owned(),
                ));
            }
        }
        if let Target::Agent(thread) = self.target
            && self.who.control
            && counted.0 > 0
        {
            let _ = shells
                .note_agent_typing(thread, self.who.user, counted.0, &self.origin)
                .await;
        }
    }

    /// The machine and the size of what is to be attached, or why it cannot be.
    async fn prepare_shell(&self, id: Id) -> Result<Ready, TerminalClosedReason> {
        let shells = self.app.engine.shells();
        let fail = |error: &dyn std::fmt::Display| {
            tracing::warn!(%error, "a shell could not be attached");
            TerminalClosedReason::LinkLost
        };
        let mut conn = self.app.db.acquire().await.map_err(|e| fail(&e))?;
        let Some(row) = rows::get(&mut conn, id).await.map_err(|e| fail(&e))? else {
            return Err(TerminalClosedReason::Exited);
        };
        if row.opened_by != self.who.user {
            // A shell is its opener's alone. A ticket is only ever given to the opener, so this is a
            // ticket for somebody else's shell, which cannot have been given.
            return Err(TerminalClosedReason::Revoked);
        }
        if !row.is_live() {
            return Err(closed_reason(&row));
        }
        if row.state != rows::state::OPEN {
            return Err(TerminalClosedReason::LinkLost);
        }
        match shells.holds(&mut conn, &row).await.map_err(|e| fail(&e))? {
            Holding::Yes => {}
            Holding::Permission | Holding::Gate(_) => {
                drop(conn);
                let _ = shells
                    .finish(id, Ending::REVOKED, &orch_db::audit::Origin::default())
                    .await;
                return Err(TerminalClosedReason::Revoked);
            }
        }
        let Some(places) = shells
            .whereabouts(&mut conn, &row)
            .await
            .map_err(|e| fail(&e))?
        else {
            return Err(TerminalClosedReason::EnvironmentStopped);
        };
        Ok(Ready {
            machine: places.machine,
            service: Service::Pty { terminal_id: id },
            size: (clamp_size(row.cols), clamp_size(row.rows)),
        })
    }

    async fn prepare_agent(&self, thread: Id) -> Result<Ready, TerminalClosedReason> {
        let shells = self.app.engine.shells();
        let fail = |error: &dyn std::fmt::Display| {
            tracing::warn!(%error, "the agent's terminal could not be attached");
            TerminalClosedReason::LinkLost
        };
        let reached = shells
            .open_agent_terminal(thread, self.who.control)
            .await
            .map_err(|e| fail(&e))?;
        let Some(machine) = reached else {
            return Err(TerminalClosedReason::LinkLost);
        };
        Ok(Ready {
            machine,
            service: Service::AgentPty { thread_id: thread },
            size: AGENT_SIZE,
        })
    }

    /// Opens the tunnel to the shell, or says why the socket closes.
    async fn open_tunnel(&self, ready: &Ready) -> Result<Tunnel, TerminalClosedReason> {
        let tickets = self.app.link.tickets();
        let link_ticket = tickets
            .terminal(ready.machine, ready.service.clone())
            .map_err(|error| {
                tracing::warn!(%error, "a ticket for a terminal could not be made");
                TerminalClosedReason::LinkLost
            })?;
        let hub = sockets(&self.app).hub(&self.app);
        match hub.open(ready.machine, &link_ticket).await {
            Ok(tunnel) => Ok(tunnel),
            Err(OpenError::Refused {
                code: OpenErrorCode::NoSuchService,
                ..
            }) => {
                // The machine has no such shell: its connector was restarted under it.
                if let Target::Shell(id) = self.target {
                    let _ = self
                        .app
                        .engine
                        .shells()
                        .finish(
                            id,
                            Ending::because(rows::reason::LINK_LOST),
                            &orch_db::audit::Origin::default(),
                        )
                        .await;
                }
                Err(TerminalClosedReason::LinkLost)
            }
            Err(OpenError::Refused {
                code: OpenErrorCode::Refused,
                ..
            }) => {
                // The machine's own policy no longer lets shells be reached.
                if let Target::Shell(id) = self.target {
                    let _ = self
                        .app
                        .engine
                        .shells()
                        .finish(id, Ending::REVOKED, &orch_db::audit::Origin::default())
                        .await;
                }
                Err(TerminalClosedReason::Revoked)
            }
            Err(error) => {
                tracing::debug!(%error, "a tunnel to a terminal could not be opened");
                Err(TerminalClosedReason::LinkLost)
            }
        }
    }

    /// Reads exactly the byte range captured by the connector when it attached this screen.
    async fn replay(&self, tunnel: &mut Tunnel) -> io::Result<Vec<u8>> {
        tokio::time::timeout(REPLAY_WAIT_MAX, async {
            let mut header = [0_u8; PTY_REPLAY_HEADER_BYTES];
            tokio::io::AsyncReadExt::read_exact(tunnel, &mut header).await?;
            let length = u64::from_be_bytes(header);
            let cap = u64::try_from(REPLAY_BYTES).unwrap_or(0);
            if length > cap {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the terminal replay exceeds its bound",
                ));
            }
            let length = usize::try_from(length).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "the terminal replay length is not representable",
                )
            })?;
            let mut replay = vec![0_u8; length];
            tokio::io::AsyncReadExt::read_exact(tunnel, &mut replay).await?;
            Ok(replay)
        })
        .await
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "the terminal replay did not arrive",
            )
        })?
    }
}

/// What a prepared attachment needs to open its tunnel and announce itself.
struct Ready {
    machine: Id,
    service: Service,
    size: (u16, u16),
}

fn clamp_size(size: i32) -> u16 {
    u16::try_from(size).unwrap_or(SIZE_MAX).clamp(1, SIZE_MAX)
}

/// The reason a closed shell's socket gives.
fn closed_reason(row: &rows::SessionRow) -> TerminalClosedReason {
    match row.close_reason.as_deref() {
        Some(rows::reason::EXITED) => TerminalClosedReason::Exited,
        Some(rows::reason::CLOSED_BY_USER) => TerminalClosedReason::ClosedByUser,
        Some(rows::reason::DETACHED) => TerminalClosedReason::Detached,
        Some(rows::reason::ENVIRONMENT_STOPPED) => TerminalClosedReason::EnvironmentStopped,
        Some(rows::reason::REVOKED) => TerminalClosedReason::Revoked,
        _ => TerminalClosedReason::LinkLost,
    }
}

/// Whether a tunnel failed because the machine gave the shell to another screen.
fn taken_by_another(error: &io::Error) -> bool {
    error.to_string().contains(TAKEN_BY_ANOTHER)
}

// ---------------------------------------------------------------- carrying bytes

/// What one pass over the tunnel did.
enum Io {
    Read(io::Result<usize>),
    Wrote(io::Result<usize>),
}

/// Reads from the tunnel and writes to it in one step, whichever can go on, so that a shell that waits
/// for its input to be taken is never left unread, and the other way round.
async fn io_step(tunnel: &mut Tunnel, buffer: &mut [u8], read: bool, write: Option<&[u8]>) -> Io {
    poll_fn(|cx| {
        if let Some(data) = write
            && let Poll::Ready(result) = Pin::new(&mut *tunnel).poll_write(cx, data)
        {
            return Poll::Ready(Io::Wrote(result));
        }
        if read {
            let mut into = ReadBuf::new(&mut *buffer);
            match Pin::new(&mut *tunnel).poll_read(cx, &mut into) {
                Poll::Ready(Ok(())) => return Poll::Ready(Io::Read(Ok(into.filled().len()))),
                Poll::Ready(Err(error)) => return Poll::Ready(Io::Read(Err(error))),
                Poll::Pending => {}
            }
        }
        Poll::Pending
    })
    .await
}

/// The attachment while it carries bytes.
struct Pump<'a> {
    attachment: &'a Attachment,
    tunnel: Tunnel,
    events: Option<mpsc::UnboundedReceiver<TunnelEvent>>,
    inbox: mpsc::Receiver<In>,
    told: mpsc::UnboundedReceiver<Told>,
    out: &'a mpsc::UnboundedSender<Out>,
    behind: Arc<Behind>,
    heard: Arc<Heard>,
    /// What was typed and the tunnel has not taken yet.
    typed: VecDeque<Bytes>,
    /// What the shell's process exited with, once the tunnel has said.
    exit: Option<i64>,
    /// Bytes typed and printed since the row was last written.
    counted: (i64, i64),
}

impl Pump<'_> {
    async fn run(&mut self) -> Ended {
        let inner = Arc::clone(self.attachment.inner());
        let mut buffer = vec![0_u8; READ_BYTES];
        let mut tick = tokio::time::interval(inner.tuning.tick);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let heartbeat = Duration::from_secs(u64::from(inner.tuning.heartbeat_secs));
        let mut last_ping = Instant::now();
        let mut last_touch = Instant::now();
        let mut last_data = Instant::now();
        loop {
            let paused = self.behind.paused();
            let writing = self.typed.front().cloned();
            let draining = self.exit.is_some();
            tokio::select! {
                biased;
                () = inner.stop.cancelled() => return Ended::GoingAway,
                told = self.told.recv() => match told {
                    Some(Told::Taken) => return Ended::Taken,
                    Some(Told::Over(reason)) => return Ended::Over(reason),
                    Some(Told::GoingAway) | None => return Ended::GoingAway,
                },
                event = next_event(&mut self.events) => match event {
                    Some(TunnelEvent::Exit { code }) => self.exit = Some(code),
                    Some(TunnelEvent::Resize { .. }) => {}
                    None => self.events = None,
                },
                input = self.inbox.recv(), if self.typed.len() < TYPED_QUEUE => {
                    if let Some(ended) = self.browser_said(input) {
                        return ended;
                    }
                }
                step = io_step(&mut self.tunnel, &mut buffer, !paused, writing.as_deref()) => match step {
                    Io::Read(Ok(0)) => return self.tunnel_ended(None),
                    Io::Read(Ok(count)) => {
                        last_data = Instant::now();
                        self.printed(&buffer[..count]);
                    }
                    Io::Read(Err(error)) => return self.tunnel_ended(Some(&error)),
                    Io::Wrote(Ok(count)) => {
                        if let Some(front) = self.typed.front_mut() {
                            front.advance(count);
                            if front.is_empty() {
                                self.typed.pop_front();
                            }
                        }
                    }
                    Io::Wrote(Err(error)) => return self.tunnel_ended(Some(&error)),
                },
                () = self.behind.resumed.notified(), if paused => {}
                () = tokio::time::sleep_until(last_data + EXIT_DRAIN), if draining && !paused => {
                    return self.exit.map_or(Ended::LinkLost, Ended::Exit);
                }
                _ = tick.tick() => {
                    if self.heard.silent_for() >= inner.tuning.silence {
                        return Ended::BrowserGone;
                    }
                    if last_ping.elapsed() >= heartbeat {
                        last_ping = Instant::now();
                        let _ = self.out.send(Out::Ping);
                    }
                    if last_touch.elapsed() >= TOUCH_EVERY {
                        last_touch = Instant::now();
                        self.touch();
                    }
                }
            }
        }
    }

    /// Writes what went through since the last time into the shell's row, without waiting for it.
    fn touch(&mut self) {
        let Target::Shell(id) = self.attachment.target else {
            return;
        };
        let (typed, printed) = std::mem::take(&mut self.counted);
        let app = self.attachment.app.clone();
        let inner = Arc::clone(self.attachment.inner());
        inner.tasks.spawn(async move {
            if let Err(error) = app.engine.shells().used(id, typed, printed).await {
                tracing::debug!(%error, "the use of a shell could not be written");
            }
        });
    }

    /// What the shell printed goes to the browser, and the browser's lag decides whether more is read.
    fn printed(&mut self, bytes: &[u8]) {
        self.counted.1 += i64::try_from(bytes.len()).unwrap_or(i64::MAX);
        self.behind.added(bytes.len());
        let _ = self.out.send(Out::Data(Bytes::copy_from_slice(bytes)));
    }

    /// What the browser sent. Some where it ends the attachment.
    fn browser_said(&mut self, input: Option<In>) -> Option<Ended> {
        match input {
            None | Some(In::Gone) => Some(Ended::BrowserGone),
            Some(In::Fault(code, reason)) => Some(Ended::Fault(code, reason)),
            Some(In::Typed(bytes)) => {
                // Only a screen that has the right to type is passed on; a watcher's keys are not.
                if self.attachment.who.control || matches!(self.attachment.target, Target::Shell(_))
                {
                    self.counted.0 += i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                    self.typed.push_back(bytes);
                }
                None
            }
            Some(In::Resize(cols, rows_)) => {
                let may = self.attachment.who.control
                    || matches!(self.attachment.target, Target::Shell(_));
                if may && (1..=SIZE_MAX).contains(&cols) && (1..=SIZE_MAX).contains(&rows_) {
                    self.tunnel.resize(cols, rows_);
                    if let Target::Shell(id) = self.attachment.target {
                        let app = self.attachment.app.clone();
                        self.attachment.inner().tasks.spawn(async move {
                            let _ = app.engine.shells().resized(id, cols, rows_).await;
                        });
                    }
                }
                None
            }
        }
    }

    /// The tunnel ended: by the end of the shell's process, by another screen taking the shell, or by the
    /// line breaking.
    fn tunnel_ended(&self, error: Option<&io::Error>) -> Ended {
        if let Some(code) = self.exit {
            return Ended::Exit(code);
        }
        match error {
            Some(error) if taken_by_another(error) => Ended::Taken,
            _ => Ended::LinkLost,
        }
    }
}

async fn next_event(
    events: &mut Option<mpsc::UnboundedReceiver<TunnelEvent>>,
) -> Option<TunnelEvent> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

// ---------------------------------------------------------------- the keeper

/// Begins the keeper: the task that ends the shells which have to end for a reason of time, place or
/// permission, and tells the screens that have them. Called once with the realtime server.
pub(crate) fn start(inner: &Arc<Inner>) {
    inner.tasks.spawn(Arc::clone(inner).keep());
}

impl Inner {
    async fn keep(self: Arc<Self>) {
        let Ok(engine) = self.engine() else {
            return;
        };
        let shells = engine.shells();
        let mut epoch = self.epoch.subscribe();
        // The epoch as it stands at the start is looked at too: a shell whose opener lost something
        // while no process was running is found by the first look.
        let mut looked_at_permissions = false;
        let mut tick = tokio::time::interval(KEEP_EVERY.max(self.tuning.tick));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let tell = |row: &rows::SessionRow, ending: Ending| {
            let reason = match ending.reason {
                rows::reason::DETACHED => TerminalClosedReason::Detached,
                rows::reason::ENVIRONMENT_STOPPED => TerminalClosedReason::EnvironmentStopped,
                _ => TerminalClosedReason::Revoked,
            };
            self.terminals.tell(row.id, Told::Over(reason));
        };
        loop {
            tokio::select! {
                () = self.stop.cancelled() => return,
                _ = tick.tick() => {
                    if let Err(error) = shells.sweep(&tell).await {
                        tracing::warn!(%error, "the shells that have to end could not be looked for");
                    }
                    if !looked_at_permissions {
                        looked_at_permissions = true;
                        if let Err(error) = shells.recheck(&tell).await {
                            tracing::warn!(%error, "the shells could not be checked against what they were opened by");
                        }
                    }
                }
                moved = epoch.changed() => {
                    if moved.is_err() {
                        return;
                    }
                    epoch.borrow_and_update();
                    if let Err(error) = shells.recheck(&tell).await {
                        tracing::warn!(%error, "the shells could not be checked against what they were opened by");
                    }
                }
            }
        }
    }
}
