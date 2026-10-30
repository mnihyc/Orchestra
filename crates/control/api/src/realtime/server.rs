//! Browser socket authentication and reauthentication.
//!
//! `/api/v1/ws` is the one socket a browser tab holds. The server speaks only
//! the subprotocol `orch.v1`: a client that offers none of it is let in and
//! closed at once with 4409, since a browser sees the code of a close and not
//! the refusal of a handshake. The first frame has to be `auth`, within ten
//! seconds, with an access token of a browser session; the answer is `ready`.
//! The token lasts a quarter of an hour and the client sends `reauth` with a
//! new one before it runs out; a connection that outlives its token is closed
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

use crate::middleware::authentication::{self, Caller, Via, token_hash};
use crate::realtime::deltas::{Next, Outbox, Tag};
use crate::realtime::subscriptions::{self, Subscription};
use crate::realtime::{Fail, Inner, chats, documents};
use crate::state::App;
use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use chrono::{DateTime, Utc};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use http::{HeaderMap, HeaderValue, header};
use orch_api_contract::common::ErrorCode;
use orch_db::queries::{feeds, identity};
use orch_realtime_contract::feeds::Feed;
use orch_realtime_contract::frames::{
    ClientFrame, ErrorFrame, PongFrame, ReadyFrame, ServerFrame, SocketErrorCode, SubId,
};
use orch_realtime_contract::pacing::RateLimit;
use orch_realtime_contract::typing::send_limit;
use orch_realtime_contract::version::{
    BacklogAction, CloseCode, MAX_CLIENT_FRAME_BYTES, SUBPROTOCOL, negotiate, outlived,
};
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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
            self.outbox.push_frame(tag, text);
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

/// `GET /api/v1/ws`: the socket every tab holds.
pub async fn socket(
    State(app): State<App>,
    peer: Option<axum::Extension<axum::extract::ConnectInfo<std::net::SocketAddr>>>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
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
        .on_upgrade(move |socket| serve(app, socket, speaks, origin))
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
struct Login {
    user: Id,
    as_admin: bool,
    session: Id,
    hash: Vec<u8>,
    expires: DateTime<Utc>,
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

/// Finds out who a token is for. Only the access token of a browser session
/// is the socket's: an API token is not.
async fn login(app: &App, token: &str) -> Result<Login, Ending> {
    let unauthenticated = || ending(CloseCode::Unauthenticated, REASON_UNAUTHENTICATED);
    let mut headers = HeaderMap::new();
    let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| unauthenticated())?;
    headers.insert(header::AUTHORIZATION, value);
    let now = app.clock.now();
    let caller = match authentication::authenticate(app, &headers, now).await {
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
    if !authed.is_session() {
        return Err(unauthenticated());
    }
    let session_id = match &authed.via {
        Via::Session { session_id } => *session_id,
        Via::Token { .. } => return Err(unauthenticated()),
    };
    let hash = token_hash(token);
    let expires = {
        let mut conn = app.db.acquire().await.map_err(internal)?;
        super::watches::session_until(&mut conn, authed.id(), session_id, &hash, now)
            .await
            .map_err(internal)?
    };
    let Some(expires) = expires else {
        return Err(unauthenticated());
    };
    Ok(Login {
        user: authed.id(),
        as_admin: authed.as_admin,
        session: session_id,
        hash,
        expires,
    })
}

#[cfg(test)]
pub(super) async fn epoch_test_connection(
    app: &App,
    inner: &Inner,
    token: &str,
) -> Result<Arc<Connection>, Ending> {
    let login = login(app, token).await?;
    Ok(Arc::new(Connection::new(
        inner,
        login.user,
        &login,
        orch_db::audit::Origin::default(),
    )))
}

/// Waits for the first frame, which has to be `auth`, until the time is up.
async fn first(app: &App, stream: &mut SplitStream<WebSocket>) -> Result<Login, Ending> {
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
            Ok(auth) => login(app, &auth.token).await,
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

async fn serve(app: App, socket: WebSocket, speaks: bool, origin: orch_db::audit::Origin) {
    let (sink, mut stream) = socket.split();
    if !speaks {
        close_socket(
            sink,
            ending(CloseCode::Unsupported, "that protocol is not spoken here"),
        )
        .await;
        return;
    }
    let login = match first(&app, &mut stream).await {
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
        access_expires_at: Time::new(login.expires),
        heartbeat_secs: inner.tuning.heartbeat_secs,
        build: app.build.to_string(),
    });
    if let Ok(text) = ready.encode() {
        connection.outbox.push_frame(
            Tag {
                id: 0,
                generation: 0,
            },
            text,
        );
    }
    let reading = read(app.clone(), Arc::clone(&connection), stream, login);
    let writing = write(sink, Arc::clone(&connection));
    tokio::pin!(reading, writing);
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
async fn write(mut sink: SplitSink<WebSocket, Message>, connection: Arc<Connection>) {
    loop {
        match connection.outbox.next().await {
            Next::Frame(text) => {
                let sent = sink.send(Message::Text(text.into())).await;
                connection.outbox.done();
                if sent.is_err() {
                    return;
                }
            }
            Next::Close(code, reason) => {
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
                        if let Err(refusal) = handle(&app, &connection, &mut login, frame).await {
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
                if outlived(Time::new(login.expires), now) {
                    break ending(CloseCode::Unauthenticated, "the access token ran out");
                }
            }
            moved = epoch.changed() => {
                if moved.is_err() {
                    return;
                }
                let now_epoch = *epoch.borrow_and_update();
                if let Err(refusal) = epoch_moved(&app, &connection, &login, now_epoch).await {
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
async fn epoch_moved(
    app: &App,
    connection: &Arc<Connection>,
    login: &Login,
    epoch: i64,
) -> Result<(), Ending> {
    let inner = &app.realtime.inner;
    {
        let mut conn = app.db.acquire().await.map_err(internal)?;
        match identity::user_by_id(&mut conn, connection.user)
            .await
            .map_err(internal)?
        {
            Some(user) if user.suspended() => {
                return Err(ending(CloseCode::Suspended, "this user is suspended"));
            }
            None => return Err(ending(CloseCode::Unauthenticated, REASON_UNAUTHENTICATED)),
            Some(_) => {}
        }
        // A session that was ended, or a token that was revoked, is not good
        // any longer, whatever time it had left.
        let good = feeds::access_token_expiry(&mut conn, &login.hash, app.clock.now())
            .await
            .map_err(internal)?;
        if good.is_none() {
            return Err(ending(CloseCode::Unauthenticated, REASON_UNAUTHENTICATED));
        }
    }
    inner
        .recheck_connection(connection, epoch)
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
            let fresh = self::login(app, &reauth.token).await?;
            if fresh.user != connection.user {
                return Err(ending(
                    CloseCode::Unauthenticated,
                    "that token is for another person",
                ));
            }
            *lock(&connection.authenticated) = fresh.clone();
            *login = fresh;
            Ok(())
        }
        ClientFrame::Sub(sub) => subscriptions::subscribe(inner, connection, sub)
            .await
            .map_err(fail),
        ClientFrame::Unsub(unsub) => {
            subscriptions::unsubscribe(inner, connection, unsub).await;
            Ok(())
        }
        ClientFrame::Visible(visible) => {
            match visible.feed() {
                Ok(feed) => inner.set_visible(connection, &feed, visible.visible),
                Err(bad) => connection.error(None, SocketErrorCode::BadFeed, &bad.to_string()),
            }
            Ok(())
        }
        ClientFrame::Typing(typing) => {
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

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("user", &self.user)
            .field("session", &self.session)
            .field("expires", &self.expires)
            .finish_non_exhaustive()
    }
}

impl Connection {
    /// The exact current session credential, without the bearer token itself.
    pub(crate) fn credential(&self) -> (Id, Vec<u8>, Time) {
        let login = lock(&self.authenticated);
        (login.session, login.hash.clone(), Time::new(login.expires))
    }

    /// The switch belongs to the current session, including after a fresh identity check.
    pub(crate) fn as_admin(&self) -> bool {
        lock(&self.authenticated).as_admin
    }

    pub(crate) fn update_admin_mode(&self, session: Id, hash: &[u8], as_admin: bool) {
        let mut login = lock(&self.authenticated);
        if login.session == session && login.hash == hash {
            login.as_admin = as_admin;
        }
    }

    /// Updates the bound expiry only if reauthentication has not replaced this credential.
    pub(crate) fn update_expiry(&self, session: Id, hash: &[u8], expires: DateTime<Utc>) {
        let mut login = lock(&self.authenticated);
        if login.session == session && login.hash == hash {
            login.expires = expires;
        }
    }

    /// Withholds content and closes an identity whose clock deadline has passed.
    /// The browser session the connection signed in with, as its last `auth` frame said.
    pub(crate) fn session(&self) -> Id {
        lock(&self.authenticated).session
    }

    pub(crate) fn authenticated_at(&self, now: Time) -> bool {
        if now >= Time::new(lock(&self.authenticated).expires) {
            self.close(CloseCode::Unauthenticated, "the access token ran out");
            return false;
        }
        self.outbox.closing().is_none()
    }
}
