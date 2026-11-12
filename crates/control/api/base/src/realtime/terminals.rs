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

use super::ticket_origin::Origin as TicketOrigin;

use crate::middleware::authentication::new_token;

use crate::middleware::authentication::token_hash;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::realtime::Inner;

use crate::registry::Ctx;

use crate::state::App;

use axum::extract::ws::CloseFrame;

use axum::extract::ws::Message;

use axum::extract::ws::WebSocket;

use axum::extract::ws::WebSocketUpgrade;

use axum::response::Response;

use bytes::Buf;

use bytes::Bytes;

use chrono::DateTime;

use chrono::Utc;

use futures::stream::SplitSink;

use futures::stream::SplitStream;

use futures::SinkExt;

use futures::StreamExt;

use orch_api_contract::terminals::TERMINAL_SUBPROTOCOL;

use orch_api_contract::terminals::TERMINAL_TICKET_SECONDS;

use orch_api_contract::terminals::TerminalAttachQuery;

use orch_api_contract::terminals::TerminalTicket;

use orch_db::queries::terminals as rows;

use orch_engine::shells::Ending;

use orch_engine::shells::Holding;

use orch_link::tunnels::channel::OpenError;

use orch_link::tunnels::channel::Tunnel;

use orch_link::tunnels::channel::TunnelEvent;

use orch_link::tunnels::terminal::TerminalHub;

use orch_link_contract::tunnels::OpenErrorCode;

use orch_link_contract::tunnels::PTY_REPLAY_HEADER_BYTES;

use orch_link_contract::tunnels::Service;

use orch_realtime_contract::terminal::Backpressure;

use orch_realtime_contract::terminal::Flow;

use orch_realtime_contract::terminal::REPLAY_BYTES;

use orch_realtime_contract::terminal::TerminalClientFrame;

use orch_realtime_contract::terminal::TerminalClosedFrame;

use orch_realtime_contract::terminal::TerminalClosedReason;

use orch_realtime_contract::terminal::TerminalExitFrame;

use orch_realtime_contract::terminal::TerminalPongFrame;

use orch_realtime_contract::terminal::TerminalReadyFrame;

use orch_realtime_contract::terminal::TerminalServerFrame;

use orch_realtime_contract::version::CloseCode;

use orch_types::Id;

use orch_types::Time;

use std::collections::HashMap;

use std::collections::VecDeque;

use std::future::Future;

use std::future::poll_fn;

use std::io;

use std::pin::Pin;

use std::sync::atomic::AtomicI64;

use std::sync::atomic::AtomicU64;

use std::sync::atomic::Ordering;

use std::sync::Arc;

use std::sync::Mutex;

use std::sync::MutexGuard;

use std::sync::PoisonError;

use std::task::Poll;

use std::time::Duration;

use tokio::io::AsyncRead;

use tokio::io::AsyncWrite;

use tokio::io::ReadBuf;

use tokio::sync::Notify;

use tokio::sync::mpsc;

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
pub const MAX_TYPED_FRAME_BYTES: usize = 256 * 1024;

/// How many frames of the browser wait for the tunnel before the socket stops reading the browser,
/// which makes the browser's own line wait: a bound on what a socket holds for a shell that takes
/// nothing.
const INBOX_FRAMES: usize = 32;

/// How many chunks of what was typed are held for the tunnel at once.
pub const TYPED_QUEUE: usize = 8;

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
#[derive(Debug, Clone)]
pub struct Ticket {
    id: Id,
    surface: orch_api_contract::surface::HttpSurface,
    origin: TicketOrigin,
    shell: Option<ShellPlace>,
    agent: Option<AgentPlace>,
    /// What it opens.
    pub door: Door,
    expires: DateTime<Utc>,
}

/// Actual physical shell facts captured only by its bound ticket producer.
#[derive(Debug, Clone, Copy)]
struct ShellPlace {
    environment: Id,
    machine: Option<Id>,
    generation: i32,
}

/// The original agent/environment association, not a replacement thread's authority.
#[derive(Debug, Clone, Copy)]
struct AgentPlace {
    environment: Id,
    agent: Option<Id>,
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
    tickets: Arc<Mutex<HashMap<Vec<u8>, Ticket>>>,
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
    pub fn issue(&self, ctx: &Ctx, door: Door) -> ApiResult<TerminalTicket> {
        let origin = TicketOrigin::capture(ctx)?;
        self.issue_at(
            ctx.app.clock.now(),
            door,
            ctx.http_surface()?,
            origin,
            None,
            None,
        )
        .publish()
    }

    pub fn issue_shell(
        &self,
        ctx: &Ctx,
        terminal: Id,
        environment: Id,
        machine: Option<Id>,
        generation: i32,
    ) -> ApiResult<TerminalTicket> {
        self.issue_shell_pending(ctx, terminal, environment, machine, generation)?
            .publish()
    }

    pub fn issue_shell_pending(
        &self,
        ctx: &Ctx,
        terminal: Id,
        environment: Id,
        machine: Option<Id>,
        generation: i32,
    ) -> ApiResult<crate::middleware::idempotency::memory::Reservation<TerminalTicket>> {
        let origin = TicketOrigin::capture(ctx)?;
        if !matches!(
            origin.operation(),
            Some("TerminalTicketCreate" | "EnvironmentTerminalCreate")
        ) {
            return Err(ApiError::internal(
                "a bound shell ticket needs its actual accepted operation",
            ));
        }
        Ok(self.issue_at(
            ctx.app.clock.now(),
            Door::Shell(terminal),
            ctx.http_surface()?,
            origin,
            Some(ShellPlace {
                environment,
                machine,
                generation,
            }),
            None,
        ))
    }

    pub fn issue_agent_pending(
        &self,
        ctx: &Ctx,
        thread: Id,
        environment: Id,
        agent: Option<Id>,
        control: bool,
    ) -> ApiResult<crate::middleware::idempotency::memory::Reservation<TerminalTicket>> {
        let origin = TicketOrigin::capture(ctx)?;
        let selection = if control {
            "AgentTerminalTicketCreate.control"
        } else {
            "AgentTerminalTicketCreate.view"
        };
        if origin.operation() != Some(selection) {
            return Err(ApiError::internal(
                "a bound agent ticket needs its actual accepted operation",
            ));
        }
        Ok(self.issue_at(
            ctx.app.clock.now(),
            Door::Agent { thread, control },
            ctx.http_surface()?,
            origin,
            None,
            Some(AgentPlace { environment, agent }),
        ))
    }

    fn issue_at(
        &self,
        now: DateTime<Utc>,
        door: Door,
        surface: orch_api_contract::surface::HttpSurface,
        origin: TicketOrigin,
        shell: Option<ShellPlace>,
        agent: Option<AgentPlace>,
    ) -> crate::middleware::idempotency::memory::Reservation<TerminalTicket> {
        let id = orch_types::new_id();
        let token = new_token("tt_");
        let expires = now + chrono::Duration::seconds(i64::from(TERMINAL_TICKET_SECONDS));
        let mut tickets = lock(&self.tickets);
        tickets.retain(|_, held| held.expires > now);
        tickets.insert(
            token_hash(&token),
            Ticket {
                id,
                surface,
                origin,
                shell,
                agent,
                door,
                expires,
            },
        );
        drop(tickets);
        let hash = token_hash(&token);
        let tickets = self.tickets.clone();
        crate::middleware::idempotency::memory::Reservation::new(
            id,
            TerminalTicket {
                ticket: token,
                ticket_expires_at: Time::new(expires),
            },
            move || {
                let mut tickets = lock(&tickets);
                if tickets.get(&hash).is_some_and(|ticket| ticket.id == id) {
                    tickets.remove(&hash);
                }
            },
        )
    }

    /// Takes the ticket `token` out, if it is one that was given and has not run out: a ticket is
    /// good once.
    pub fn redeem(
        &self,
        now: DateTime<Utc>,
        token: &str,
        surface: orch_api_contract::surface::HttpSurface,
    ) -> Option<Ticket> {
        let mut tickets = lock(&self.tickets);
        let hash = token_hash(token);
        if tickets
            .get(&hash)
            .is_none_or(|held| held.surface != surface)
        {
            return None;
        }
        tickets.remove(&hash).filter(|held| held.expires > now)
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
    Frame(TerminalServerFrame),
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
    authority: Attachment,
) {
    while let Some(item) = out.recv().await {
        let sent = match item {
            Out::Data(bytes) => {
                let count = bytes.len();
                let (accepted, reason) =
                    send_native(&mut sink, &authority, Message::Binary(bytes)).await;
                behind.handed_over(count);
                if !accepted {
                    close_authority(&mut sink, reason).await;
                    break;
                }
                Ok(())
            }
            Out::Frame(frame) => {
                let protected = matches!(
                    frame,
                    TerminalServerFrame::Ready(_) | TerminalServerFrame::Exit(_)
                );
                let message =
                    Message::Text(serde_json::to_string(&frame).unwrap_or_default().into());
                if protected {
                    let (accepted, reason) = send_native(&mut sink, &authority, message).await;
                    if !accepted {
                        close_authority(&mut sink, reason).await;
                        break;
                    }
                    Ok(())
                } else {
                    sink.send(message).await
                }
            }
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

/// Existing output capacity is awaited before the real owner checkpoint. Only exact
/// accepted frame bytes may flush later; the key/session deadline is synchronous at start_send.
async fn send_native(
    sink: &mut SplitSink<WebSocket, Message>,
    authority: &Attachment,
    message: Message,
) -> (bool, TerminalClosedReason) {
    let mut reason = TerminalClosedReason::Revoked;
    let accepted = super::publication::send(sink, authority.app.clock.as_ref(), async {
        match authority.current_actor().await {
            Ok((_, caller, session)) => Ok(Some((message, (caller, session)))),
            Err(why) => {
                reason = why;
                Ok(None)
            }
        }
    })
    .await;
    (matches!(accepted, Ok(true)), reason)
}

async fn close_authority(sink: &mut SplitSink<WebSocket, Message>, reason: TerminalClosedReason) {
    let notice = TerminalServerFrame::Closed(TerminalClosedFrame { reason });
    let _ = sink
        .send(Message::Text(
            serde_json::to_string(&notice).unwrap_or_default().into(),
        ))
        .await;
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code: NORMAL,
            reason: String::new().into(),
        })))
        .await;
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
    let ticket = ctx
        .surface
        .and_then(|surface| sockets(&app).redeem(app.clock.now(), &query.ticket, surface));
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
                user: ticket.origin.user(),
                control,
            };
            let attachment = Attachment {
                app: app.clone(),
                target,
                who,
                origin,
                credential: ticket.origin,
                shell: ticket.shell,
                agent: ticket.agent,
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
#[derive(Clone)]
struct Attachment {
    app: App,
    target: Target,
    who: Who,
    origin: orch_db::audit::Origin,
    credential: TicketOrigin,
    shell: Option<ShellPlace>,
    agent: Option<AgentPlace>,
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
    Out::Frame(frame.clone())
}

fn closed(reason: TerminalClosedReason) -> Out {
    frame(&TerminalServerFrame::Closed(TerminalClosedFrame { reason }))
}

/// The socket's close code for an orderly end: the frame before it said why.
const NORMAL: u16 = 1000;

impl Attachment {
    /// One effect-free, coherent current boundary for this actual retained door. It never
    /// closes the native shell because a key changed; the existing opener keeper owns that.
    /// The returned input capability can narrow an agent control stream to its existing view.
    async fn current_actor(
        &self,
    ) -> Result<(bool, orch_authz::Caller, Option<Time>), TerminalClosedReason> {
        let mut conn = self
            .app
            .db
            .begin_snapshot()
            .await
            .map_err(|_| TerminalClosedReason::LinkLost)?;
        let Some((caller, session_deadline)) = self
            .credential
            .restore_current(&mut conn, self.app.clock.as_ref())
            .await
            .map_err(|_| TerminalClosedReason::LinkLost)?
        else {
            return Err(TerminalClosedReason::Revoked);
        };
        let (control, checked) = match self.target {
            Target::Shell(id) => {
                let Some(row) = rows::get(&mut conn, id)
                    .await
                    .map_err(|_| TerminalClosedReason::LinkLost)?
                else {
                    return Err(TerminalClosedReason::Exited);
                };
                if row.opened_by != self.who.user {
                    return Err(TerminalClosedReason::Revoked);
                }
                if let Some(physical) = self.shell {
                    let Some(site) = rows::site(&mut conn, row.environment_id)
                        .await
                        .map_err(|_| TerminalClosedReason::LinkLost)?
                    else {
                        return Err(TerminalClosedReason::EnvironmentStopped);
                    };
                    if row.environment_id != physical.environment
                        || row.env_generation != physical.generation
                        || site.generation != physical.generation
                        || site.machine_id != physical.machine
                    {
                        return Err(TerminalClosedReason::EnvironmentStopped);
                    }
                }
                let Some((principal, manage)) = self
                    .credential
                    .principal(
                        &mut conn,
                        caller,
                        row.environment_id,
                        match self.credential.operation() {
                            Some("EnvironmentTerminalCreate") => "EnvironmentTerminalCreate",
                            Some("TerminalTicketCreate") | None => "TerminalTicketCreate",
                            Some(_) => return Err(TerminalClosedReason::Revoked),
                        },
                        self.app.clock.as_ref(),
                    )
                    .await
                    .map_err(|_| TerminalClosedReason::LinkLost)?
                else {
                    return Err(TerminalClosedReason::Revoked);
                };
                if !manage
                    || !orch_authz::environments::stays_open(
                        &mut conn,
                        &principal,
                        &orch_authz::environments::Shell {
                            environment: row.environment_id,
                            thread: row.thread_id,
                            opened_by: row.opened_by,
                        },
                        orch_authz::Policy::default(),
                    )
                    .await
                    .map_err(|_| TerminalClosedReason::LinkLost)?
                {
                    return Err(TerminalClosedReason::Revoked);
                }
                (true, principal.caller().clone())
            }
            Target::Agent(thread) => {
                let Some(row) = orch_db::queries::threads::thread(&mut conn, thread)
                    .await
                    .map_err(|_| TerminalClosedReason::LinkLost)?
                else {
                    return Err(TerminalClosedReason::Exited);
                };
                let Some(environment) = row.environment_id.filter(|_| row.deleted_at.is_none())
                else {
                    return Err(TerminalClosedReason::EnvironmentStopped);
                };
                if self.agent.is_some_and(|place| {
                    place.environment != environment || place.agent != row.agent_id
                }) {
                    return Err(TerminalClosedReason::Revoked);
                }
                let selection = if self.who.control {
                    "AgentTerminalTicketCreate.control"
                } else {
                    "AgentTerminalTicketCreate.view"
                };
                let Some((principal, scope_control)) = self
                    .credential
                    .principal(
                        &mut conn,
                        caller,
                        environment,
                        selection,
                        self.app.clock.as_ref(),
                    )
                    .await
                    .map_err(|_| TerminalClosedReason::LinkLost)?
                else {
                    return Err(TerminalClosedReason::Revoked);
                };
                let object = orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id: environment,
                };
                if !orch_authz::resources::check(
                    &mut conn,
                    &principal,
                    object,
                    orch_access_contract::permissions::Permission::TerminalView,
                    orch_authz::Policy::default(),
                )
                .await
                .map_err(|_| TerminalClosedReason::LinkLost)?
                .is_allowed()
                {
                    return Err(TerminalClosedReason::Revoked);
                }
                let control = self.who.control
                    && scope_control
                    && orch_authz::resources::check(
                        &mut conn,
                        &principal,
                        object,
                        orch_access_contract::permissions::Permission::TerminalManage,
                        orch_authz::Policy::default(),
                    )
                    .await
                    .map_err(|_| TerminalClosedReason::LinkLost)?
                    .is_allowed();
                (control, principal.caller().clone())
            }
        };
        conn.commit()
            .await
            .map_err(|_| TerminalClosedReason::LinkLost)?;
        if !super::ticket_origin::deadline_allows(
            &checked,
            session_deadline,
            self.app.clock.as_ref(),
        ) {
            return Err(TerminalClosedReason::Revoked);
        }
        Ok((control, checked, session_deadline))
    }

    async fn current(&self) -> Result<bool, TerminalClosedReason> {
        self.current_actor().await.map(|(control, _, _)| control)
    }

    fn inner(&self) -> &Arc<Inner> {
        &self.app.realtime.inner
    }

    async fn serve(self, sink: SplitSink<WebSocket, Message>, stream: SplitStream<WebSocket>) {
        let behind = Arc::new(Behind::new());
        let (out, outgoing) = mpsc::unbounded_channel();
        let writer = tokio::spawn(write(sink, outgoing, Arc::clone(&behind), self.clone()));
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
        if let Err(reason) = self.current().await {
            goodbye(reason);
            return;
        }
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
        let ready = Ready {
            machine: places.machine,
            service: Service::Pty { terminal_id: id },
            size: (clamp_size(row.cols), clamp_size(row.rows)),
        };
        drop(conn);
        self.current().await?;
        Ok(ready)
    }

    async fn prepare_agent(&self, thread: Id) -> Result<Ready, TerminalClosedReason> {
        self.current().await?;
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
    Refused(TerminalClosedReason),
    ViewOnly,
}

/// Only unfinished asynchronous restoration; no completed proof survives a wait.
type WriteCheck<'a> = Pin<
    Box<
        dyn Future<Output = Result<(bool, orch_authz::Caller, Option<Time>), TerminalClosedReason>>
            + Send
            + 'a,
    >,
>;

/// Reads from the tunnel and writes to it in one step, whichever can go on, so that a shell that waits
/// for its input to be taken is never left unread, and the other way round.
async fn io_step<'a>(
    tunnel: &mut Tunnel,
    buffer: &mut [u8],
    read: bool,
    write: Option<&[u8]>,
    authority: &'a Attachment,
    authorization: &mut Option<WriteCheck<'a>>,
) -> Io {
    poll_fn(|cx| {
        if let Some(data) = write {
            let checked = match authorization
                .get_or_insert_with(|| Box::pin(authority.current_actor()))
                .as_mut()
                .poll(cx)
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(checked) => checked,
            };
            // A completed proof is consumed now; it never stays cached across a wait.
            *authorization = None;
            match checked {
                Err(reason) => return Poll::Ready(Io::Refused(reason)),
                Ok((false, _, _)) => return Poll::Ready(Io::ViewOnly),
                Ok((true, caller, session_deadline)) => {
                    if !super::ticket_origin::deadline_allows(
                        &caller,
                        session_deadline,
                        authority.app.clock.as_ref(),
                    ) {
                        return Poll::Ready(Io::Refused(TerminalClosedReason::Revoked));
                    }
                    // The current transaction ended before this synchronous queue boundary.
                    match Pin::new(&mut *tunnel).poll_write(cx, data) {
                        Poll::Ready(result) => return Poll::Ready(Io::Wrote(result)),
                        Poll::Pending => {
                            // None of these bytes was accepted. The next native wake must
                            // complete a NEW check, not reuse the completed proof above.
                            *authorization = Some(Box::pin(authority.current_actor()));
                        }
                    }
                }
            }
        } else {
            *authorization = None;
        }
        // Only an unpolled next check can remain here; no read transaction spans
        // native data I/O. Output retains its own final current-Read publisher.
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
        let mut permission_epoch = inner.epoch.subscribe();
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let heartbeat = Duration::from_secs(u64::from(inner.tuning.heartbeat_secs));
        let mut last_ping = Instant::now();
        let mut last_touch = Instant::now();
        let mut last_data = Instant::now();
        let mut last_authority = Instant::now();
        let mut authorization: Option<WriteCheck<'_>> = None;
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
                event = next_event(&mut self.events) => {
                    authorization = None;
                    match event {
                        Some(TunnelEvent::Exit { code }) => self.exit = Some(code),
                        Some(TunnelEvent::Resize { .. }) => {}
                        None => self.events = None,
                    }
                },
                changed = permission_epoch.changed() => {
                    // The selected authority epoch invalidates unfinished old work.
                    authorization = None;
                    if changed.is_err() { return Ended::GoingAway; }
                    permission_epoch.borrow_and_update();
                    match self.attachment.current().await {
                        Ok(false) => self.typed.clear(),
                        Ok(true) => {},
                        Err(reason) => return Ended::Over(reason),
                    }
                    last_authority = Instant::now();
                },
                input = self.inbox.recv(), if self.typed.len() < TYPED_QUEUE => {
                    // Finish/drop the old DB work before another selected-message check
                    // or resize can wait on the pool or touch the native channel.
                    authorization = None;
                    let current_control = match self.attachment.current().await {
                        Ok(control) => control,
                        Err(reason) => return Ended::Over(reason),
                    };
                    if let Some(ended) = self.browser_said(input, current_control) {
                        return ended;
                    }
                }
                step = io_step(&mut self.tunnel, &mut buffer, !paused, writing.as_deref(), self.attachment, &mut authorization) => match step {
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
                    Io::Refused(reason) => return Ended::Over(reason),
                    Io::ViewOnly => self.typed.clear(),
                },
                () = self.behind.resumed.notified(), if paused => {}
                () = tokio::time::sleep_until(last_data + EXIT_DRAIN), if draining && !paused => {
                    return self.exit.map_or(Ended::LinkLost, Ended::Exit);
                }
                _ = tick.tick() => {
                    // Reuse the existing keeper's one-second cadence for idle key expiry or
                    // personal changes that do not announce a shared authorization epoch.
                    // Input, every resumed write and output delivery keep their own boundaries.
                    if last_authority.elapsed() >= KEEP_EVERY {
                        // A tick must not cancel/restart an in-flight restore, nor hold
                        // its connection while waiting for a second check on the pool.
                        // Complete that existing check and discard its result as a write
                        // proof; the next native write still obtains its own fresh check.
                        let current = match authorization.take() {
                            Some(mut pending) => pending.as_mut().await.map(|(control, _, _)| control),
                            None => self.attachment.current().await,
                        };
                        match current {
                            Ok(false) => self.typed.clear(),
                            Ok(true) => {},
                            Err(reason) => return Ended::Over(reason),
                        }
                        last_authority = Instant::now();
                    }
                    if self.heard.silent_for() >= inner.tuning.silence {
                        return Ended::BrowserGone;
                    }
                    if last_ping.elapsed() >= heartbeat {
                        last_ping = Instant::now();
                        let _ = self.out.send(Out::Ping);
                    }
                    if last_touch.elapsed() >= TOUCH_EVERY && authorization.is_none() {
                        // Defer DB housekeeping while an unfinished restore owns a pool slot.
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
    fn browser_said(&mut self, input: Option<In>, current_control: bool) -> Option<Ended> {
        match input {
            None | Some(In::Gone) => Some(Ended::BrowserGone),
            Some(In::Fault(code, reason)) => Some(Ended::Fault(code, reason)),
            Some(In::Typed(bytes)) => {
                // Only a screen that has the right to type is passed on; a watcher's keys are not.
                if current_control
                    && (self.attachment.who.control
                        || matches!(self.attachment.target, Target::Shell(_)))
                {
                    self.counted.0 += i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                    self.typed.push_back(bytes);
                }
                None
            }
            Some(In::Resize(cols, rows_)) => {
                let may = current_control
                    && (self.attachment.who.control
                        || matches!(self.attachment.target, Target::Shell(_)));
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

#[cfg(any(test, feature = "test-support"))]
pub mod surface_tests {

    use super::*;

    use orch_api_contract::surface::HttpSurface;
    #[cfg(any(test, feature = "test-support"))]
    fn surface_terminal_ticket_keeps_its_original_door_until_matching_redemption() {
        for surface in [HttpSurface::Browser, HttpSurface::Key] {
            let sockets = Sockets::default();
            let user = orch_types::new_id();
            let door = Door::Shell(orch_types::new_id());
            let now = Utc::now();
            let via = crate::middleware::authentication::Via::Session {
                session_id: orch_types::new_id(),
            };
            let origin =
                TicketOrigin::from_caller(&orch_authz::Caller::session(user), &via).unwrap();
            let ticket = sockets
                .issue_at(now, door, surface, origin, None, None)
                .publish()
                .unwrap();
            let foreign = if surface == HttpSurface::Browser {
                HttpSurface::Key
            } else {
                HttpSurface::Browser
            };
            assert!(sockets.redeem(now, &ticket.ticket, foreign).is_none());
            let kept = sockets
                .redeem(now, &ticket.ticket, surface)
                .expect("wrong plane must not consume a terminal ticket");
            assert_eq!(kept.surface, surface);
            assert_eq!(kept.origin.user(), user);
            assert_eq!(kept.door, door);
            assert!(sockets.redeem(now, &ticket.ticket, surface).is_none());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_surface_terminal_ticket_keeps_its_original_door_until_matching_redemption() {
        self::surface_terminal_ticket_keeps_its_original_door_until_matching_redemption()
    }
}
