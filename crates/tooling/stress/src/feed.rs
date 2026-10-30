//! The realtime side: one socket as a browser tab holds it (`/api/v1/ws`, subprotocol `orch.v1`), and what the drive
//! learns from its frames: when a change reached it (the live feed's lag) and when a turn ended.
//!
//! A socket says who it is with `auth`, follows feeds with `sub` and leaves them with `unsub`, renews its token with
//! `reauth`, and pings at the beat `ready` gave. A dropped socket is opened again and follows the same feeds; a
//! `resync` is answered by following the feed again from now (the drive does not keep rows, so it has nothing to
//! repair). Lag is measured from the moment the writer sent its request to the first frame of each socket that carries
//! the change: the input's `client_id` on a thread's feed, the inserted block's id on a document's feed.

use crate::api::Person;
use crate::metrics::Recorder;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_util::sync::CancellationToken;

/// How long a sent change is remembered for: a change not seen by then is not going to be measured.
const REMEMBER: Duration = Duration::from_secs(300);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Changes sent and not yet forgotten, by the key their feed shows them by.
#[derive(Default)]
pub struct LagBook {
    sent: Mutex<HashMap<String, Instant>>,
}

impl LagBook {
    /// A change is being sent now.
    pub fn mark(&self, key: &str) {
        let mut sent = lock(&self.sent);
        if sent.len() > 50_000 {
            sent.retain(|_, at| at.elapsed() < REMEMBER);
        }
        sent.insert(key.to_owned(), Instant::now());
    }

    /// How long ago the change of `key` was sent, if this tool sent it.
    pub fn age(&self, key: &str) -> Option<Duration> {
        lock(&self.sent).get(key).map(Instant::elapsed)
    }
}

/// How a turn ended.
#[derive(Debug, Clone)]
pub struct TurnEnd {
    /// `completed`, `failed`, `interrupted`, or `blocked` when the thread stopped before it.
    pub state: String,
    /// The failure's code, or why the thread is blocked.
    pub code: Option<String>,
}

#[derive(Default)]
struct TurnInner {
    waiting: HashMap<String, (String, oneshot::Sender<TurnEnd>)>,
    client_of_input: HashMap<String, String>,
    client_of_thread: HashMap<String, String>,
    ended_early: HashMap<String, TurnEnd>,
}

/// The inputs that wait for their turn to end, found by their `client_id`, their id and their thread.
#[derive(Default)]
pub struct TurnBook {
    inner: Mutex<TurnInner>,
}

impl TurnBook {
    /// An input with `client_id` is about to be sent to `thread`: the receiver hears how its turn ended.
    pub fn expect(&self, thread: &str, client_id: &str) -> oneshot::Receiver<TurnEnd> {
        let (tx, rx) = oneshot::channel();
        let mut inner = lock(&self.inner);
        inner
            .waiting
            .insert(client_id.to_owned(), (thread.to_owned(), tx));
        inner
            .client_of_thread
            .insert(thread.to_owned(), client_id.to_owned());
        rx
    }

    /// The input of `client_id` has the id `input_id` (from its reply or its row on a feed).
    pub fn input_known(&self, client_id: &str, input_id: &str) {
        let mut inner = lock(&self.inner);
        if !inner.waiting.contains_key(client_id) {
            return;
        }
        inner
            .client_of_input
            .insert(input_id.to_owned(), client_id.to_owned());
        if let Some(end) = inner.ended_early.remove(input_id) {
            Self::finish(&mut inner, client_id, end);
        }
    }

    /// A turn row in a terminal state for the input `input_id`.
    pub fn turn_ended(&self, input_id: &str, end: TurnEnd) {
        let mut inner = lock(&self.inner);
        match inner.client_of_input.get(input_id).cloned() {
            Some(client_id) => Self::finish(&mut inner, &client_id, end),
            None => {
                if inner.ended_early.len() > 10_000 {
                    inner.ended_early.clear();
                }
                inner.ended_early.insert(input_id.to_owned(), end);
            }
        }
    }

    /// The thread says it is blocked: the input waiting on it will not get a turn until it is not.
    pub fn thread_blocked(&self, thread: &str, why: String) {
        let mut inner = lock(&self.inner);
        if let Some(client_id) = inner.client_of_thread.get(thread).cloned() {
            Self::finish(
                &mut inner,
                &client_id,
                TurnEnd {
                    state: "blocked".to_owned(),
                    code: Some(why),
                },
            );
        }
    }

    /// The input is no longer waited for.
    pub fn forget(&self, client_id: &str) {
        let mut inner = lock(&self.inner);
        if let Some((thread, _)) = inner.waiting.remove(client_id)
            && inner
                .client_of_thread
                .get(&thread)
                .is_some_and(|c| c == client_id)
        {
            inner.client_of_thread.remove(&thread);
        }
        inner.client_of_input.retain(|_, c| c != client_id);
    }

    fn finish(inner: &mut TurnInner, client_id: &str, end: TurnEnd) {
        if let Some((thread, tx)) = inner.waiting.remove(client_id) {
            let _ = tx.send(end);
            if inner
                .client_of_thread
                .get(&thread)
                .is_some_and(|c| c == client_id)
            {
                inner.client_of_thread.remove(&thread);
            }
        }
        inner.client_of_input.retain(|_, c| c != client_id);
    }
}

/// What every socket reports to.
pub struct Books {
    pub lag: LagBook,
    pub turns: TurnBook,
    pub rec: Arc<Recorder>,
}

/// A socket's handle: frames to send. Dropping every handle ends it.
#[derive(Clone)]
pub struct Socket {
    out: mpsc::UnboundedSender<Value>,
}

impl Socket {
    /// Follows `feed` under the number `id`.
    pub fn sub(&self, id: i64, feed: &str) {
        let _ = self
            .out
            .send(json!({ "op": "sub", "id": id, "feed": feed }));
    }

    /// Stops following the feed of `id`.
    pub fn unsub(&self, id: i64) {
        let _ = self.out.send(json!({ "op": "unsub", "id": id }));
    }

    /// Says that the feed's view is in front of the person, as a tab does.
    pub fn visible(&self, feed: &str) {
        let _ = self
            .out
            .send(json!({ "op": "visible", "feed": feed, "visible": true }));
    }
}

/// Opens a socket as `who` (it connects in the background, and again whenever it drops, until `stop`).
pub fn open(url: String, who: Arc<Person>, books: Arc<Books>, stop: CancellationToken) -> Socket {
    let (out, rx) = mpsc::unbounded_channel();
    tokio::spawn(run(url, who, books, rx, stop));
    Socket { out }
}

/// The first sightings of keys on one socket: a row arrives again with every change of it, and is measured once.
struct Seen {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl Seen {
    fn first(&mut self, key: &str) -> bool {
        if self.set.contains(key) {
            return false;
        }
        if self.order.len() >= 20_000
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        self.set.insert(key.to_owned());
        self.order.push_back(key.to_owned());
        true
    }
}

async fn run(
    url: String,
    who: Arc<Person>,
    books: Arc<Books>,
    mut rx: mpsc::UnboundedReceiver<Value>,
    stop: CancellationToken,
) {
    let rec = books.rec.clone();
    // ORCH_STRESS_DEBUG=1 prints each socket's connections, readiness, errors and closings.
    let debug = std::env::var_os("ORCH_STRESS_DEBUG").is_some();
    let say = |what: &str| {
        if debug {
            eprintln!(
                "{} socket of {}: {what}",
                chrono::Utc::now().format("%H:%M:%S%.3f"),
                who.login
            );
        }
    };
    // The feeds followed, by number: sent again on every new connection.
    let mut subs: BTreeMap<i64, String> = BTreeMap::new();
    let mut seen = Seen {
        set: HashSet::new(),
        order: VecDeque::new(),
    };
    let mut first = true;
    while !stop.is_cancelled() {
        if !first {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
        first = false;
        let mut request = match url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => {
                rec.note(format!("the socket address {url} is not usable: {e}"));
                return;
            }
        };
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("orch.v1"),
        );
        let stream = match tokio_tungstenite::connect_async(request).await {
            Ok((stream, _)) => stream,
            Err(e) => {
                say(&format!("connect failed: {e}"));
                rec.socket("connect failed");
                continue;
            }
        };
        say("connected");
        rec.socket("connected");
        let (mut sink, mut source) = stream.split();
        let auth = json!({ "op": "auth", "token": who.token() }).to_string();
        if sink.send(Message::text(auth)).await.is_err() {
            rec.socket("dropped");
            continue;
        }
        let mut tokens = who.tokens();
        let mut ready = false;
        let mut beat = tokio::time::interval(Duration::from_secs(25));
        beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut pings: HashMap<i64, Instant> = HashMap::new();
        let mut ping_n: i64 = 0;
        loop {
            tokio::select! {
                () = stop.cancelled() => {
                    let _ = sink.send(Message::Close(None)).await;
                    return;
                }
                incoming = source.next() => {
                    let text = match incoming {
                        Some(Ok(Message::Text(text))) => text,
                        Some(Ok(Message::Close(frame))) => {
                            say(&format!("closed by the server: {frame:?}"));
                            let code = frame.map_or_else(|| "none".to_owned(), |f| u16::from(f.code).to_string());
                            rec.socket(&format!("closed by the server ({code})"));
                            break;
                        }
                        Some(Ok(_)) => continue,
                        Some(Err(e)) => {
                            say(&format!("dropped: {e}"));
                            rec.socket("dropped");
                            break;
                        }
                        None => {
                            say("dropped: the stream ended");
                            rec.socket("dropped");
                            break;
                        }
                    };
                    let Ok(frame) = serde_json::from_str::<Value>(text.as_str()) else {
                        rec.socket("unreadable frame");
                        continue;
                    };
                    let op = frame.get("op").and_then(Value::as_str).unwrap_or("?");
                    rec.socket(&format!("frame {op}"));
                    match op {
                        "ready" => {
                            say(&format!("ready; following {} feeds", subs.len()));
                            ready = true;
                            let secs = frame.get("heartbeat_secs").and_then(Value::as_u64).unwrap_or(25).max(5);
                            beat = tokio::time::interval(Duration::from_secs(secs));
                            beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                            for (id, feed) in &subs {
                                let sub = json!({ "op": "sub", "id": id, "feed": feed }).to_string();
                                if sink.send(Message::text(sub)).await.is_err() {
                                    break;
                                }
                            }
                        }
                        "sync" => read_sync(&frame, &who.id, &books, &mut seen),
                        "resync" => {
                            rec.socket("resync asked");
                            if let Some(id) = frame.get("id").and_then(Value::as_i64)
                                && let Some(feed) = subs.get(&id) {
                                    let sub = json!({ "op": "sub", "id": id, "feed": feed }).to_string();
                                    let _ = sink.send(Message::text(sub)).await;
                                }
                        }
                        "error" => {
                            say(&format!("error frame: {frame}"));
                            let code = frame.get("code").and_then(Value::as_str).unwrap_or("?");
                            rec.socket(&format!("error {code}"));
                        }
                        "closed" => {
                            let reason = frame.get("reason").and_then(Value::as_str).unwrap_or("?");
                            if reason != "unsubscribed" {
                                rec.socket(&format!("feed closed: {reason}"));
                            }
                        }
                        "pong" => {
                            if let Some(at) = frame.get("n").and_then(Value::as_i64).and_then(|n| pings.remove(&n)) {
                                rec.ping(at.elapsed());
                            }
                        }
                        _ => {}
                    }
                }
                outgoing = rx.recv() => {
                    let Some(frame) = outgoing else {
                        say("closing: nothing holds it any more");
                        let _ = sink.send(Message::Close(None)).await;
                        return;
                    };
                    let id = frame.get("id").and_then(Value::as_i64);
                    match (frame.get("op").and_then(Value::as_str), id) {
                        (Some("sub"), Some(id)) => {
                            let feed = frame.get("feed").and_then(Value::as_str).unwrap_or_default().to_owned();
                            subs.insert(id, feed);
                        }
                        (Some("unsub"), Some(id))
                            if subs.remove(&id).is_none() => {
                                continue;
                            }
                        _ => {}
                    }
                    if ready && sink.send(Message::text(frame.to_string())).await.is_err() {
                        rec.socket("dropped");
                        break;
                    }
                }
                changed = tokens.changed() => {
                    if changed.is_ok() && ready {
                        let reauth = json!({ "op": "reauth", "token": who.token() }).to_string();
                        let _ = sink.send(Message::text(reauth)).await;
                    }
                }
                _ = beat.tick() => {
                    if ready {
                        ping_n += 1;
                        pings.insert(ping_n, Instant::now());
                        let ping = json!({ "op": "ping", "n": ping_n }).to_string();
                        let _ = sink.send(Message::text(ping)).await;
                    }
                }
            }
        }
    }
}

fn text_at<'a>(row: &'a Value, pointer: &str) -> Option<&'a str> {
    row.pointer(pointer).and_then(Value::as_str)
}

/// What a sync frame says to the drive: inputs seen (lag; their ids for the turns waited on), turns that ended,
/// threads that are blocked, document blocks seen (lag).
fn read_sync(frame: &Value, me: &str, books: &Books, seen: &mut Seen) {
    let Some(data) = frame.get("data") else {
        return;
    };
    if let Some(inputs) = data.get("inputs").and_then(Value::as_array) {
        for row in inputs {
            let (Some(client_id), Some(input_id)) =
                (text_at(row, "/client_id"), text_at(row, "/id"))
            else {
                continue;
            };
            books.turns.input_known(client_id, input_id);
            if seen.first(client_id)
                && let Some(age) = books.lag.age(client_id)
            {
                let own = text_at(row, "/author/id") == Some(me);
                let kind = if own {
                    "thread input, the writer's own tab"
                } else {
                    "thread input, another person's tab"
                };
                books.rec.lag(kind, age);
            }
        }
    }
    if let Some(turns) = data.get("turns").and_then(Value::as_array) {
        for row in turns {
            let state = text_at(row, "/state").unwrap_or_default();
            if !matches!(state, "completed" | "failed" | "interrupted") {
                continue;
            }
            if let Some(input_id) = text_at(row, "/input_id") {
                let code = text_at(row, "/error/code").map(str::to_owned);
                books.turns.turn_ended(
                    input_id,
                    TurnEnd {
                        state: state.to_owned(),
                        code,
                    },
                );
            }
        }
    }
    if let Some(thread) = data.get("thread")
        && text_at(thread, "/state") == Some("blocked")
        && let Some(id) = text_at(thread, "/id")
    {
        let why = thread
            .get("blocked")
            .map(|b| {
                text_at(b, "/reason")
                    .or_else(|| text_at(b, "/kind"))
                    .map_or_else(|| b.to_string(), str::to_owned)
            })
            .unwrap_or_default();
        books
            .turns
            .thread_blocked(id, why.chars().take(160).collect());
    }
    if let Some(blocks) = data.get("blocks").and_then(Value::as_array) {
        for row in blocks {
            if let Some(id) = text_at(row, "/id")
                && seen.first(id)
                && let Some(age) = books.lag.age(id)
            {
                books.rec.lag("document block", age);
            }
        }
    }
}
