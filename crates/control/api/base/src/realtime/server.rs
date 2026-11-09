//! Browser socket authentication and reauthentication.
//!
//! `/api/v1/ws` serves browser sessions; `/openapi/v1/ws` serves account keys through
//! the same protocol implementation. The mount pins the kind for initial auth and reauth.
//! The server speaks only
//! the subprotocol `orch.v1`: a client that offers none of it is let in and
//! closed at once with 4409, since a browser sees the code of a close and not
//! the refusal of a handshake. The first frame has to be `auth`, within ten
//! seconds, with a browser access token or an existing account API key; the answer is `ready`.
//! A browser keeps its actual access-token deadline; a lasting key has no fabricated
//! deadline. The client sends the same `reauth` frame before an actual credential end;
//! a connection that outlives its credential is closed
//! with 4401. A `reauth` that is good is not answered, since no frame exists
//! for it and a second `ready` would make the browser follow every feed
//! again; one that is not good, or that is another person's, closes the
//! connection with 4401, which the browser answers by renewing its token.
//!
//! After that the connection reads frames and writes what its outbox holds,
//! at the same time. A connection that hears nothing for a minute is closed:
//! the client pings every 25 seconds, and a minute is hundreds of round trips
//! of a slow line, so that slowness alone never closes one. When the
//! authorization epoch moves, the person is looked at again (a suspension
//! closes the connection with 4403, a token that was revoked with 4401) and so
//! is every subscription it holds.

use crate::middleware::authentication;

use crate::middleware::authentication::Caller;

use crate::middleware::authentication::Via;

use crate::middleware::authentication::token_hash;

use crate::realtime::deltas::Next;

use crate::realtime::deltas::Outbox;

use crate::realtime::deltas::Tag;

use crate::realtime::subscriptions;

use crate::realtime::subscriptions::Subscription;

use crate::realtime::Fail;

use crate::realtime::Inner;

use crate::realtime::chats;

use crate::realtime::documents;

use crate::state::App;

use axum::extract::State;

use axum::extract::ws::CloseFrame;

use axum::extract::ws::Message;

use axum::extract::ws::WebSocket;

use axum::extract::ws::WebSocketUpgrade;

use axum::response::Response;

use chrono::DateTime;

use chrono::Utc;

use futures::stream::SplitSink;

use futures::stream::SplitStream;

use futures::Sink;

use futures::SinkExt;

use futures::StreamExt;

use http::HeaderMap;

use http::HeaderValue;

use http::header;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::surface::HttpSurface;

use orch_db::queries::transaction_guard::Credential;

use orch_realtime_contract::feeds::Feed;

use orch_realtime_contract::frames::ClientFrame;

use orch_realtime_contract::frames::ErrorFrame;

use orch_realtime_contract::frames::PongFrame;

use orch_realtime_contract::frames::ReadyFrame;

use orch_realtime_contract::frames::ServerFrame;

use orch_realtime_contract::frames::SocketErrorCode;

use orch_realtime_contract::frames::SubId;

use orch_realtime_contract::pacing::RateLimit;

use orch_realtime_contract::typing::send_limit;

use orch_realtime_contract::version::BacklogAction;

use orch_realtime_contract::version::CloseCode;

use orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES;

use orch_realtime_contract::version::SUBPROTOCOL;

use orch_realtime_contract::version::negotiate;

use orch_types::Id;

use orch_types::Time;

use std::collections::HashMap;

use std::collections::HashSet;

use std::sync::Arc;

use std::sync::Mutex;

use std::sync::MutexGuard;

use std::sync::PoisonError;

use tokio::time::MissedTickBehavior;
/// What a socket is closed with when something the server stands on failed:
/// 1011, "internal error". The client connects again after a pause.
const INTERNAL_ERROR: u16 = 1011;

/// What a client may send beyond its limit before the socket itself refuses
/// the message: the limit is checked on the frame and answered 4400, so the
/// socket's own must be above it. Four times the limit is room for a frame that
/// is too large to say how large, and little enough to cost nothing.
const SOCKET_MESSAGE_FACTOR: usize = 4;

/// The words of a close, which are for a person who is looking for the cause.
const REASON_MALFORMED: &str = "that is not a frame of the protocol";

const REASON_UNAUTHENTICATED: &str = "authentication is missing, invalid or expired";

/// One browser tab's connection.
#[derive(Debug)]
pub struct Connection {
    pub(crate) number: u64,
    /// Its name, which `ready` gives the client.
    pub(crate) name: String,
    /// The person.
    pub(crate) user: Id,
    /// The resolved origin of the socket handshake, retained for the actions on this connection.
    pub(crate) origin: orch_db::audit::Origin,
    /// What waits for the socket.
    pub(crate) outbox: Outbox,
    subs: Mutex<HashMap<SubId, Arc<Subscription>>>,
    shown: Mutex<HashSet<Feed>>,
    /// How soon a `typing` frame is let through, for each feed it is sent on.
    typing: Mutex<HashMap<Feed, RateLimit>>,
    authenticated: Mutex<Login>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Connection {
    fn new(inner: &Inner, user: Id, login: &Login, origin: orch_db::audit::Origin) -> Self {
        let number = inner.number();
        metrics::gauge!("orch_ws_connections","feed_kind"=>"all").increment(1.0);
        Self {
            number,
            name: format!("c{}", &orch_types::new_id().simple().to_string()[24..]),
            user,
            origin,
            outbox: Outbox::new(
                Arc::clone(&inner.clock),
                inner.tuning.buffer_max_bytes,
                inner.tuning.buffer_max_millis,
            ),
            subs: Mutex::new(HashMap::new()),
            shown: Mutex::new(HashSet::new()),
            typing: Mutex::new(HashMap::new()),
            authenticated: Mutex::new(login.clone()),
        }
    }

    /// Ends the connection with `code` after the frame the socket is taking.
    pub fn close(&self, code: CloseCode, reason: &str) {
        self.outbox.close(code.code(), reason);
    }

    /// Says that a request could not be served; the connection stays open.
    pub(crate) fn error(&self, id: Option<SubId>, code: SocketErrorCode, detail: &str) {
        let frame = ServerFrame::Error(ErrorFrame {
            id,
            code,
            detail: detail.to_owned(),
        });
        if let Ok(text) = frame.encode() {
            let tag = Tag {
                id: id.unwrap_or_default(),
                generation: 0,
            };
            self.outbox.push_control(tag, text);
        }
    }

    /// How many subscriptions it holds.
    pub(crate) fn held(&self) -> u32 {
        u32::try_from(lock(&self.subs).len()).unwrap_or(u32::MAX)
    }

    pub(crate) fn add(&self, sub: &Arc<Subscription>) {
        lock(&self.subs).insert(sub.tag().id, Arc::clone(sub));
    }

    /// Takes away the subscription with this number, if it has one.
    pub(crate) fn take(&self, id: SubId) -> Option<Arc<Subscription>> {
        lock(&self.subs).remove(&id)
    }

    /// Takes `sub` away, if it is still the one held under its number.
    pub(crate) fn forget(&self, sub: &Arc<Subscription>) {
        let mut subs = lock(&self.subs);
        if subs
            .get(&sub.tag().id)
            .is_some_and(|held| Arc::ptr_eq(held, sub))
        {
            subs.remove(&sub.tag().id);
        }
    }

    pub(crate) fn subscriptions(&self) -> Vec<Arc<Subscription>> {
        lock(&self.subs).values().cloned().collect()
    }

    /// Whether it follows `feed` under any number.
    pub(crate) fn follows(&self, feed: &Feed) -> bool {
        lock(&self.subs).values().any(|sub| sub.feed() == feed)
    }

    /// Whether a `typing` frame on `feed` at `now` is let through: not one that
    /// comes sooner after the last than the protocol lets a client send.
    pub(crate) fn may_type(&self, feed: &Feed, now: Time) -> bool {
        lock(&self.typing)
            .entry(feed.clone())
            .or_insert_with(send_limit)
            .admit(now)
    }

    /// The feeds this connection must withdraw from the other processes when it ends.
    pub(crate) fn visible_feeds(&self) -> Vec<Feed> {
        lock(&self.shown).iter().cloned().collect()
    }

    /// Notes that its person says `feed` is, or is not, in front of them.
    /// Whether that changed what was noted.
    pub(crate) fn mark_visible(&self, feed: &Feed, visible: bool) -> bool {
        let mut shown = lock(&self.shown);
        if visible {
            shown.insert(feed.clone())
        } else {
            shown.remove(feed)
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        metrics::gauge!("orch_ws_connections","feed_kind"=>"all").decrement(1.0);
    }
}

// ---------------------------------------------------------------- the handshake

/// The subprotocols a client offered, in the order it offered them.
fn offered(headers: &HeaderMap) -> Vec<String> {
    headers
        .get_all(header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|each| each.trim().to_owned())
        .filter(|each| !each.is_empty())
        .collect()
}

/// General realtime socket on the explicitly mounted browser or account-key surface.
pub async fn socket(
    State(app): State<App>,
    axum::Extension(surface): axum::Extension<HttpSurface>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Err(error) = authentication::on_surface(surface, &headers) {
        return error.into_response_for(&orch_types::new_id().to_string());
    }
    let origin = orch_db::audit::Origin {
        as_admin: None,
        ip: peer.map(|peer| {
            orch_config::client_address::resolve_observed(
                peer.0.0.ip(),
                &headers,
                &app.config.api.trusted_proxy_cidrs,
                app.clock.now().timestamp(),
            )
        }),
        user_agent: headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(|value| {
                value
                    .chars()
                    .take(crate::sign_in::USER_AGENT_MAX_CHARS)
                    .collect()
            }),
        request_id: Some(orch_types::new_id().to_string()),
    };
    let offers = offered(&headers);
    let names: Vec<&str> = offers.iter().map(String::as_str).collect();
    let speaks = negotiate(&names).is_ok();
    // A client that offers none of the protocol is answered with the first it
    // offered and closed with 4409: a browser fails the handshake of one that
    // is answered with no protocol at all, and then never sees the code.
    let chosen = if speaks {
        vec![SUBPROTOCOL.to_owned()]
    } else {
        offers.first().cloned().into_iter().collect()
    };
    upgrade
        .protocols(chosen)
        .max_message_size(MAX_CLIENT_FRAME_BYTES * SOCKET_MESSAGE_FACTOR)
        .max_frame_size(MAX_CLIENT_FRAME_BYTES * SOCKET_MESSAGE_FACTOR)
        .on_upgrade(move |socket| serve(app, socket, speaks, origin, surface))
}

// ---------------------------------------------------------------- reading

enum Heard {
    /// A frame of the protocol.
    Frame(ClientFrame),
    /// Something that is not, and the code to close with.
    Fault(CloseCode, &'static str),
    /// The line says the client is there, and says nothing else.
    Alive,
    /// The client closed, or the line broke.
    Ended,
}

async fn hear(stream: &mut SplitStream<WebSocket>) -> Heard {
    match stream.next().await {
        None | Some(Ok(Message::Close(_))) => Heard::Ended,
        Some(Err(error)) => {
            // A message over what the socket takes is a frame that is too large.
            let words = error.to_string().to_lowercase();
            if words.contains("too long") || words.contains("limit") {
                Heard::Fault(CloseCode::Malformed, REASON_MALFORMED)
            } else {
                Heard::Ended
            }
        }
        Some(Ok(Message::Ping(_) | Message::Pong(_))) => Heard::Alive,
        Some(Ok(Message::Binary(_))) => Heard::Fault(CloseCode::Malformed, REASON_MALFORMED),
        Some(Ok(Message::Text(text))) => match ClientFrame::decode(text.as_str()) {
            Ok(frame) => Heard::Frame(frame),
            Err(fault) => Heard::Fault(fault.close_code(), REASON_MALFORMED),
        },
    }
}

// ---------------------------------------------------------------- who is speaking

/// A person who proved who they are with an access token.
#[derive(Clone)]
pub struct Login {
    surface: HttpSurface,
    pub(super) user: Id,
    pub(super) as_admin: bool,
    pub(super) via: Via,
    pub(super) captured: orch_authz::CapturedCaller,
    /// Always unadmitted: a shared operation checkpoint admits each read separately.
    pub(super) current: orch_authz::Caller,
    pub(super) hash: Vec<u8>,
    pub(super) expires: Option<DateTime<Utc>>,
    pub(super) authentication: Id,
    pub(super) decision: Id,
}

impl Login {
    pub(super) fn id(&self) -> Id {
        match self.via {
            Via::OwnerRead { ref source } => source.source(),
            Via::Session { session_id } => session_id,
            Via::Token { token_id, .. } => token_id,
        }
    }

    pub(super) fn browser_session(&self) -> Option<Id> {
        match self.via {
            Via::Session { session_id } => Some(session_id),
            Via::Token { .. } | Via::OwnerRead { .. } => None,
        }
    }

    pub(super) fn credential(&self) -> crate::middleware::requests::ApiResult<Credential<'_>> {
        Ok(match self.via {
            Via::OwnerRead { .. } => {
                return Err(crate::middleware::requests::ApiError::new(
                    ErrorCode::AuthSessionRequired,
                    "Native owner reads do not open HTTP or realtime credentials",
                ));
            }
            Via::Session { session_id } => Credential::Session {
                id: session_id,
                hash: &self.hash,
            },
            Via::Token { token_id, .. } => Credential::Token {
                id: token_id,
                hash: &self.hash,
            },
        })
    }
}

/// Current facts from this socket's real credential, with the accepted key ceiling retained.
/// Browser mode is live session context; it is not a native Session lease frozen at handshake.
pub(super) async fn current_identity(
    app: &App,
    conn: &mut orch_db::Conn,
    before: &Login,
) -> crate::middleware::requests::ApiResult<crate::middleware::transaction_guard::CurrentHttpIdentity>
{
    use crate::middleware::{requests::ApiError, transaction_guard};
    let mut identity =
        transaction_guard::current_read_identity(app, conn, before.user, before.credential()?)
            .await?;
    if identity.user.user.suspended() {
        return Err(ApiError::new(
            ErrorCode::AuthSuspended,
            "this user is suspended",
        ));
    }
    if before.browser_session().is_none() {
        let accepted = before
            .captured
            .restore_current(conn, app.clock.as_ref())
            .await
            .map_err(crate::middleware::authorization::restoration_error)?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::AuthInvalidToken,
                    "the accepted key is no longer valid",
                )
            })?;
        identity.caller = accepted
            .intersect_restrictions(&identity.caller)
            .map_err(ApiError::internal)?;
    }
    identity.user.as_admin = identity.caller.as_admin();
    identity.now = app.clock.time();
    identity.check_deadline(app)?;
    Ok(identity)
}

fn identity_ending(error: crate::middleware::requests::ApiError) -> Ending {
    match error.code() {
        ErrorCode::AuthSuspended => ending(CloseCode::Suspended, "this user is suspended"),
        ErrorCode::AuthInvalidToken | ErrorCode::AuthRequired => unauthenticated(),
        _ => internal(error),
    }
}

/// How a connection ends when it is the server's doing.
type Ending = (u16, String);

fn ending(code: CloseCode, reason: &str) -> Ending {
    (code.code(), reason.to_owned())
}

fn internal(error: impl std::fmt::Display) -> Ending {
    tracing::warn!(%error, "a realtime connection failed");
    (INTERNAL_ERROR, "something went wrong".to_owned())
}

/// Whether nothing was heard since `last_heard` for as long as `silence`. The
/// protocol's own rule is a minute; it is the server's setting so that a
/// test need not wait for one.
fn heard_nothing_for(last_heard: Time, now: Time, silence: std::time::Duration) -> bool {
    let waited = now.unix_millis().saturating_sub(last_heard.unix_millis());
    u128::try_from(waited).is_ok_and(|waited| waited >= silence.as_millis())
}

fn unauthenticated() -> Ending {
    ending(CloseCode::Unauthenticated, REASON_UNAUTHENTICATED)
}

/// Authenticates the socket's browser access token or existing account key.
async fn login(app: &App, token: &str, surface: HttpSurface) -> Result<Login, Ending> {
    let mut headers = HeaderMap::new();
    let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| unauthenticated())?;
    headers.insert(header::AUTHORIZATION, value);
    let now = app.clock.now();
    let caller = match authentication::authenticate_on(surface, app, &headers, now).await {
        Ok(caller) => caller,
        Err(error) => {
            return Err(match error.code() {
                ErrorCode::AuthSuspended => ending(CloseCode::Suspended, "this user is suspended"),
                ErrorCode::AuthInvalidToken => unauthenticated(),
                _ => internal(error),
            });
        }
    };
    let Caller::User(authed) = caller else {
        return Err(unauthenticated());
    };
    let hash = token_hash(token);
    let caller = crate::middleware::authorization::caller_of(&authed).map_err(internal)?;
    let mut login = Login {
        surface,
        user: authed.id(),
        as_admin: caller.as_admin(),
        via: authed.via.clone(),
        captured: orch_authz::CapturedCaller::capture(&caller),
        current: caller,
        hash,
        expires: None,
        authentication: orch_types::new_id(),
        decision: orch_types::new_id(),
    };
    let mut tx = app.db.begin().await.map_err(internal)?;
    let identity = current_identity(app, &mut tx, &login)
        .await
        .map_err(identity_ending)?;
    tx.commit().await.map_err(internal)?;
    identity.check_deadline(app).map_err(identity_ending)?;
    login.as_admin = identity.caller.as_admin();
    login.expires = earlier(
        identity.expires_at.map(Into::into),
        identity
            .caller
            .grant()
            .and_then(|grant| grant.expires_at())
            .map(Into::into),
    );
    login.via = identity.user.via;
    login.current = identity.caller;
    Ok(login)
}

#[cfg(any(test, feature = "test-support"))]
pub(super) async fn epoch_test_connection(
    app: &App,
    inner: &Inner,
    token: &str,
) -> Result<Arc<Connection>, Ending> {
    let surface = HttpSurface::for_token(token).ok_or_else(unauthenticated)?;
    let login = login(app, token, surface).await?;
    Ok(Arc::new(Connection::new(
        inner,
        login.user,
        &login,
        orch_db::audit::Origin::default(),
    )))
}

/// Waits for the first frame, which has to be `auth`, until the time is up.
async fn first(
    app: &App,
    stream: &mut SplitStream<WebSocket>,
    surface: HttpSurface,
) -> Result<Login, Ending> {
    let inner = &app.realtime.inner;
    let waited = tokio::time::timeout(inner.tuning.auth_timeout, async {
        loop {
            match hear(stream).await {
                Heard::Alive => {}
                other => return other,
            }
        }
    })
    .await;
    match waited {
        Err(_) => Err(ending(
            CloseCode::Unauthenticated,
            "no auth within the time allowed",
        )),
        Ok(Heard::Ended) => Err(ending(CloseCode::GoingAway, "")),
        Ok(Heard::Fault(code, reason)) => Err(ending(code, reason)),
        Ok(Heard::Alive) => Err(ending(CloseCode::Malformed, REASON_MALFORMED)),
        Ok(Heard::Frame(frame)) => match frame.first() {
            Ok(auth) => login(app, &auth.token, surface).await,
            Err(code) => Err(ending(code, "the first frame has to be auth")),
        },
    }
}

// ---------------------------------------------------------------- the connection

async fn close_socket(mut sink: SplitSink<WebSocket, Message>, ending: Ending) {
    let (code, reason) = ending;
    let _ = sink
        .send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.into(),
        })))
        .await;
    let _ = sink.close().await;
}

async fn serve(
    app: App,
    socket: WebSocket,
    speaks: bool,
    origin: orch_db::audit::Origin,
    surface: HttpSurface,
) {
    let (sink, mut stream) = socket.split();
    if !speaks {
        close_socket(
            sink,
            ending(CloseCode::Unsupported, "that protocol is not spoken here"),
        )
        .await;
        return;
    }
    let login = match first(&app, &mut stream, surface).await {
        Ok(login) => login,
        Err(refusal) => {
            close_socket(sink, refusal).await;
            return;
        }
    };
    app.realtime.attach_link(app.link.clone());
    app.realtime.attach_app(&app);
    let inner = Arc::clone(&app.realtime.inner);
    let connection = Arc::new(Connection::new(&inner, login.user, &login, origin));
    inner
        .registry()
        .connections
        .insert(connection.number, Arc::clone(&connection));
    let ready = ServerFrame::Ready(ReadyFrame {
        connection_id: connection.name.clone(),
        user_id: login.user,
        server_time: inner.clock.time(),
        access_expires_at: login.expires.map(Time::new),
        heartbeat_secs: inner.tuning.heartbeat_secs,
        build: app.build.to_string(),
    });
    if let Ok(text) = ready.encode() {
        connection.outbox.push_control(
            Tag {
                id: 0,
                generation: 0,
            },
            text,
        );
    }
    let mut reading = Box::pin(read(app.clone(), Arc::clone(&connection), stream, login));
    let mut writing = Box::pin(write(sink, Arc::clone(&connection), Arc::clone(&inner)));
    tokio::select! {
        () = &mut reading => {
            // The reader has decided: the writer is given a moment to say why.
            let _ = tokio::time::timeout(inner.tuning.close_grace, &mut writing).await;
        }
        () = &mut writing => {}
    }
    for sub in connection.subscriptions() {
        let mut st = sub.state.lock().await;
        st.open = false;
        sub.set_live(false);
        inner.unregister(&sub);
        connection.forget(&sub);
    }
    inner.presence_ended(&connection);
    inner.registry().connections.remove(&connection.number);
}

/// What the socket is sent: whatever the outbox holds, a frame at a time, and
/// at the end the close that was decided.
fn start_frame<S: Sink<Message> + Unpin>(
    sink: &mut S,
    inner: &Inner,
    connection: &Connection,
    pending: super::deltas::PendingFrame,
    text: String,
    origin: &Login,
    cut: Option<(&Subscription, i64, Id)>,
) -> Result<bool, S::Error> {
    connection.outbox.accept(pending, || {
        // Keep replacement authentication serialized with physical acceptance too.
        let current = lock(&connection.authenticated);
        if current.authentication != origin.authentication
            || current.decision != origin.decision
            || current
                .expires
                .is_some_and(|end| Time::new(end) <= inner.clock.time())
            || !current.current.key_deadline_allows(inner.clock.as_ref())
            || cut.is_some_and(|(sub, epoch, decision)| {
                sub.decided() != epoch || current.decision != decision
            })
        {
            return Ok(false);
        }
        // Queue cancellation and the physical sink acceptance are one synchronous cut.
        std::pin::Pin::new(sink).start_send(Message::Text(text.into()))?;
        Ok(true)
    })
}

async fn discard_resource(connection: &Connection, pending: super::deltas::PendingFrame) {
    if pending.resource
        && let Some(sub) = connection
            .subscriptions()
            .into_iter()
            .find(|sub| Some(sub.tag()) == pending.tag)
    {
        sub.state.lock().await.resource_read = None;
    }
}

async fn write(
    sink: SplitSink<WebSocket, Message>,
    connection: Arc<Connection>,
    inner: Arc<Inner>,
) {
    #[cfg(any(test, feature = "test-support"))]
    let mut sink = super::origin_read_tests::capacity_sink(sink, &connection.name);
    #[cfg(not(any(test, feature = "test-support")))]
    let mut sink = sink;
    loop {
        // Keep queued projections cancellable while the real sink waits for capacity.
        connection.outbox.available().await;
        if futures::future::poll_fn(|cx| std::pin::Pin::new(&mut sink).poll_ready(cx))
            .await
            .is_err()
        {
            return;
        }
        let Some((next, pending)) = connection.outbox.take_pending() else {
            continue;
        };
        match next {
            Next::Frame(text) => {
                #[cfg(any(test, feature = "test-support"))]
                super::origin_read_tests::selected_frame(&connection.name, &text);
                let Some(pending) = pending else {
                    connection
                        .outbox
                        .close(INTERNAL_ERROR, "a pending frame lost its owner");
                    connection.outbox.done();
                    continue;
                };
                match super::watches::delivery_guard(&inner, &connection).await {
                    Ok(Some(())) => {}
                    Ok(None) => {
                        discard_resource(&connection, pending).await;
                        connection.outbox.done();
                        continue;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "a socket publication identity could not be refreshed");
                        connection
                            .outbox
                            .close(INTERNAL_ERROR, "the socket read could not be refreshed");
                        connection.outbox.done();
                        continue;
                    }
                }
                let mut refresh = None;
                let sent = if let Some(tag) = pending.tag {
                    let sub = connection
                        .subscriptions()
                        .into_iter()
                        .find(|sub| sub.tag() == tag);
                    let Some(sub) = sub else {
                        connection.outbox.done();
                        continue;
                    };
                    let mut state = sub.state.lock().await;
                    let epoch = match super::sync::read_epoch(&inner).await {
                        Ok(epoch) => epoch,
                        Err(error) => {
                            tracing::warn!(%error, "a socket publication epoch could not be read");
                            connection
                                .outbox
                                .close(INTERNAL_ERROR, "the socket read could not be refreshed");
                            connection.outbox.done();
                            continue;
                        }
                    };
                    let allowed = if pending.event
                        && matches!(sub.feed(), orch_realtime_contract::feeds::Feed::User)
                    {
                        Box::pin(inner.recheck_personal(&sub, &mut state, epoch)).await
                    } else {
                        Box::pin(inner.recheck(&sub, &mut state, epoch)).await
                    };
                    match allowed {
                        Ok(true) => {}
                        Ok(false) => {
                            if pending.resource {
                                state.resource_read = None;
                            }
                            connection.outbox.done();
                            continue;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "a socket subscription could not be refreshed");
                            connection
                                .outbox
                                .close(INTERNAL_ERROR, "the socket read could not be refreshed");
                            connection.outbox.done();
                            continue;
                        }
                    }
                    #[cfg(any(test, feature = "test-support"))]
                    super::origin_read_tests::before_start(&connection.name, &text).await;
                    let origin = connection.credential();
                    // The async subscription lock covers final acceptance, not accepted flush.
                    let sent = start_frame(
                        &mut sink,
                        &inner,
                        &connection,
                        pending,
                        text,
                        &origin,
                        Some((&sub, epoch, state.publication_identity())),
                    );
                    if matches!(&sent, Ok(false)) && pending.resource {
                        state.resource_read = None;
                    }
                    drop(state);
                    if matches!(&sent, Ok(false)) {
                        // A natural deadline may cross at the synchronous cut. Reuse the
                        // subscription's existing refresh/reset path after releasing its lock.
                        refresh = Some(sub);
                    }
                    sent
                } else {
                    let origin = connection.credential();
                    start_frame(&mut sink, &inner, &connection, pending, text, &origin, None)
                };
                match sent {
                    Ok(true) => {
                        // Accepted bytes are aftermath: later revocation cannot unsend or replay them.
                        let flushed = sink.flush().await;
                        connection.outbox.done();
                        if flushed.is_err() {
                            return;
                        }
                    }
                    Ok(false) => {
                        connection.outbox.done();
                        if let Some(sub) = refresh {
                            let _ = sub.is_live();
                        }
                        #[cfg(any(test, feature = "test-support"))]
                        super::origin_read_tests::retired(&connection.name);
                    }
                    Err(_) => {
                        connection.outbox.done();
                        return;
                    }
                }
            }
            Next::Close(code, reason) => {
                #[cfg(any(test, feature = "test-support"))]
                let sink = sink.into_inner();
                close_socket(sink, (code, reason)).await;
                return;
            }
        }
    }
}

/// What the socket sends: the frames of the client, and the clocks of the
/// connection between them.
async fn read(
    app: App,
    connection: Arc<Connection>,
    mut stream: SplitStream<WebSocket>,
    mut login: Login,
) {
    let inner = Arc::clone(&app.realtime.inner);
    let mut epoch = inner.epoch.subscribe();
    let mut tick = tokio::time::interval(inner.tuning.tick);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut last_heard = inner.clock.time();
    let end = loop {
        if connection.outbox.closing().is_some() {
            return;
        }
        tokio::select! {
            heard = hear(&mut stream) => {
                last_heard = inner.clock.time();
                match heard {
                    Heard::Ended => break ending(CloseCode::GoingAway, ""),
                    Heard::Fault(code, reason) => break ending(code, reason),
                    Heard::Alive => {}
                    Heard::Frame(frame) => {
                        if let Err(refusal) = Box::pin(handle(&app, &connection, &mut login, frame)).await {
                            break refusal;
                        }
                    }
                }
            }
            _ = tick.tick() => {
                let now = inner.clock.time();
                if connection.outbox.tick() == BacklogAction::Close {
                    // The outbox closed the connection itself, with 4408.
                    return;
                }
                if heard_nothing_for(last_heard, now, inner.tuning.silence) {
                    break ending(CloseCode::GoingAway, "nothing was heard for a minute");
                }
                if !connection.authenticated_at(now) { return; }
            }
            moved = epoch.changed() => {
                if moved.is_err() {
                    return;
                }
                let now_epoch = *epoch.borrow_and_update();
                if let Err(refusal) = Box::pin(epoch_moved(&app, &connection, now_epoch)).await {
                    break refusal;
                }
            }
            () = inner.stop.cancelled() => break ending(CloseCode::GoingAway, "the server is stopping"),
        }
    };
    connection.outbox.close(end.0, &end.1);
}

/// The authorization epoch moved: the person is looked at again, and so is
/// every feed they follow.
async fn epoch_moved(app: &App, connection: &Arc<Connection>, epoch: i64) -> Result<(), Ending> {
    let inner = &app.realtime.inner;
    if !super::watches::credential_current(inner, connection)
        .await
        .map_err(internal)?
    {
        return Err(connection.outbox.closing().unwrap_or_else(unauthenticated));
    }
    Box::pin(inner.recheck_connection(connection, epoch))
        .await
        .map_err(internal)
}

/// What a frame of the client asks. A refusal is the end of the connection.
async fn handle(
    app: &App,
    connection: &Arc<Connection>,
    login: &mut Login,
    frame: ClientFrame,
) -> Result<(), Ending> {
    let inner = &app.realtime.inner;
    match frame {
        // The connection said who it is already.
        ClientFrame::Auth(_) => Err(ending(CloseCode::Malformed, REASON_MALFORMED)),
        ClientFrame::Reauth(reauth) => {
            let mut fresh = self::login(app, &reauth.token, login.surface).await?;
            if fresh.user != connection.user {
                return Err(ending(
                    CloseCode::Unauthenticated,
                    "that token is for another person",
                ));
            }
            // A key connection retains its accepted ceiling across same-key reauthentication.
            // Switching the credential kind/key is a separate connection, never a way to shed it.
            if login.browser_session().is_none() || fresh.browser_session().is_none() {
                if login.browser_session().is_some() != fresh.browser_session().is_some()
                    || login.id() != fresh.id()
                {
                    return Err(ending(
                        CloseCode::Unauthenticated,
                        "reauthentication changed the credential origin",
                    ));
                }
                let mut conn = app.db.acquire().await.map_err(internal)?;
                fresh.captured = login.captured.clone();
                let identity = current_identity(app, &mut conn, &fresh)
                    .await
                    .map_err(identity_ending)?;
                fresh.current = identity.caller;
                fresh.via = identity.user.via;
                fresh.as_admin = fresh.current.as_admin();
                fresh.expires = earlier(
                    identity.expires_at.map(Into::into),
                    fresh
                        .current
                        .grant()
                        .and_then(|grant| grant.expires_at())
                        .map(Into::into),
                );
            }
            *lock(&connection.authenticated) = fresh.clone();
            *login = fresh;
            #[cfg(any(test, feature = "test-support"))]
            super::origin_read_tests::reauthenticated(&connection.name).await;
            let epoch = *inner.epoch.borrow();
            Box::pin(inner.recheck_connection(connection, epoch))
                .await
                .map_err(internal)
        }
        ClientFrame::Sub(sub) => Box::pin(subscriptions::subscribe(inner, connection, sub))
            .await
            .map_err(fail),
        ClientFrame::Unsub(unsub) => {
            subscriptions::unsubscribe(inner, connection, unsub).await;
            Ok(())
        }
        ClientFrame::Visible(visible) => {
            if connection.credential().current.grant().is_some() {
                connection.error(
                    None,
                    SocketErrorCode::Forbidden,
                    "this key does not admit presence publication",
                );
                return Ok(());
            }
            match visible.feed() {
                Ok(feed) => inner.set_visible(connection, &feed, visible.visible),
                Err(bad) => connection.error(None, SocketErrorCode::BadFeed, &bad.to_string()),
            }
            Ok(())
        }
        ClientFrame::Typing(typing) => {
            if connection.credential().current.grant().is_some() {
                connection.error(
                    None,
                    SocketErrorCode::Forbidden,
                    "this key does not admit typing publication",
                );
                return Ok(());
            }
            match typing.feed() {
                Ok(feed @ Feed::Doc(_)) => documents::typing(inner, connection, &feed, typing.at),
                Ok(Feed::Chat(_)) => chats::typing(inner, connection, &typing).await,
                // Nothing else typed is carried on the feeds of this server.
                Ok(_) => {}
                Err(bad) => connection.error(None, SocketErrorCode::BadFeed, &bad.to_string()),
            }
            Ok(())
        }
        ClientFrame::Ping(ping) => {
            if let Ok(text) = ServerFrame::Pong(PongFrame { n: ping.n }).encode() {
                connection.outbox.push_urgent(text);
            }
            Ok(())
        }
    }
}

fn fail(error: Fail) -> Ending {
    internal(error)
}

// The current credential is checked at each reader's delivery boundary.

fn earlier(left: Option<DateTime<Utc>>, right: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("user", &self.user)
            .field("credential", &self.id())
            .field("expires", &self.expires)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// A credential snapshot contains hashes/policy, never its bearer secret.
    pub(super) fn credential(&self) -> Login {
        lock(&self.authenticated).clone()
    }

    pub(crate) fn as_admin(&self) -> bool {
        lock(&self.authenticated).as_admin
    }

    /// Cache decisions depend on policy and origin, not merely an account or session UUID.
    pub(crate) fn decision_identity(&self) -> Id {
        lock(&self.authenticated).decision
    }

    pub(crate) fn browser_session(&self) -> Option<Id> {
        lock(&self.authenticated).browser_session()
    }

    pub(super) fn still_current(&self, before: &Login) -> bool {
        lock(&self.authenticated).authentication == before.authentication
    }

    pub(super) fn update_current(
        &self,
        before: &Login,
        identity: &crate::middleware::transaction_guard::CurrentHttpIdentity,
    ) -> bool {
        let mut login = lock(&self.authenticated);
        if login.authentication != before.authentication {
            return false;
        }
        let caller = identity.caller.clone();
        let as_admin = caller.as_admin();
        let expires = earlier(
            identity.expires_at.map(Into::into),
            caller
                .grant()
                .and_then(|grant| grant.expires_at())
                .map(Into::into),
        );
        if login.current != caller || login.expires != expires || login.as_admin != as_admin {
            login.decision = orch_types::new_id();
        }
        login.via = identity.user.via.clone();
        login.current = caller;
        login.expires = expires;
        login.as_admin = as_admin;
        if login.browser_session().is_some() {
            login.captured = orch_authz::CapturedCaller::capture(&login.current);
        }
        true
    }

    pub(crate) fn authenticated_at(&self, now: Time) -> bool {
        let expires = { lock(&self.authenticated).expires };
        if expires.is_some_and(|end| now >= Time::new(end)) {
            self.close(CloseCode::Unauthenticated, "the credential ran out");
            return false;
        }
        self.outbox.closing().is_none()
    }
}
