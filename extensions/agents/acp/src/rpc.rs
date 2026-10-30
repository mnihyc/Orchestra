//! JSON-RPC 2.0 in lines over a program's standard streams: how an Agent Client Protocol agent is spoken to.
//!
//! One JSON object for each line of the agent's standard input and output, each with the member
//! `"jsonrpc": "2.0"`. [`connect`] starts the two tasks that read and write them over any
//! pair of streams (a child process's, or a pipe in a test) and returns an [`Rpc`] to call with and a
//! receiver of everything the harness says of its own accord ([`Incoming`]).
//!
//! A call waits for its answer under a deadline and is cancel-safe: a caller that stops waiting leaves the
//! request in flight and its answer is dropped when it arrives. When the harness closes its output every call
//! in flight fails at once with [`RpcError::Closed`] and the receiver ends with [`Incoming::Closed`].

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use orch_agent_contract::flow;
use orch_types::Json;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Notify, mpsc, oneshot};

/// How long a request waits for its answer: a little under the sixty seconds the engine allows a command, so
/// that a harness that does not answer is reported by the adapter itself before its caller gives up.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(50);

/// How long the first request, `initialize`, waits: a program that has to unpack itself can take a while,
/// and nothing else can happen before it.
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(120);

/// The longest line read from the harness: 128 MiB. The longest legitimate line is the history of a long
/// conversation in one answer; a line beyond this is a harness gone wrong, and reading it whole would only
/// use up the memory of the machine.
pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;

/// The version member every frame carries.
const JSONRPC: &str = "2.0";

/// The error code of a request the harness does not know (JSON-RPC: method not found).
pub const METHOD_NOT_FOUND: i64 = -32601;

/// The error code of a request whose parameters are not what the method takes (JSON-RPC: invalid params).
pub const INVALID_PARAMS: i64 = -32602;

/// The error code of a request that failed inside the adapter (JSON-RPC: internal error).
pub const INTERNAL_ERROR: i64 = -32603;

/// Which way a frame went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// From the harness to the adapter.
    In,
    /// From the adapter to the harness.
    Out,
}

impl Direction {
    /// The word the neutral raw frame uses for it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

/// Sees every frame that goes either way, for the raw frames a session may keep.
pub type FrameTap =
    Arc<dyn Fn(Direction, &Json) -> futures::future::BoxFuture<'static, bool> + Send + Sync>;

/// The identifier of a request, which either side may choose and the other repeats in its answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RequestId {
    /// A number, as the app-server counts its own requests.
    Number(i64),
    /// A text.
    Text(String),
}

impl RequestId {
    /// The identifier as a frame writes it.
    #[must_use]
    pub fn to_json(&self) -> Json {
        match self {
            Self::Number(number) => json!(number),
            Self::Text(text) => json!(text),
        }
    }

    /// Reads the identifier of a frame.
    #[must_use]
    pub fn from_json(value: &Json) -> Option<Self> {
        match value {
            Json::Number(number) => number.as_i64().map(Self::Number),
            Json::String(text) => Some(Self::Text(text.clone())),
            _ => None,
        }
    }
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Text(text) => f.write_str(text),
        }
    }
}

/// Why a call did not give an answer.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RpcError {
    /// The connection to the harness is gone.
    #[error("the harness closed its connection: {0}")]
    Closed(String),
    /// The harness answered with an error.
    #[error("the harness refused {method} ({code}): {message}")]
    Remote {
        /// What was asked.
        method: String,
        /// The error code.
        code: i64,
        /// What the harness said.
        message: String,
        /// Anything further it said.
        data: Option<Box<Json>>,
    },
    /// No answer came in time.
    #[error("the harness did not answer {method} within {seconds} seconds")]
    Timeout {
        /// What was asked.
        method: String,
        /// How long it waited.
        seconds: u64,
    },
    /// The answer is not of the shape the adapter expects.
    #[error("the answer to {method} is not what was expected: {detail}")]
    Shape {
        /// What was asked.
        method: String,
        /// What is wrong.
        detail: String,
    },
}

/// What the harness says of its own accord.
#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    /// A notification: something happened.
    Notification {
        /// What kind.
        method: String,
        /// Its parameters.
        params: Json,
    },
    /// A request the harness makes of the adapter and waits for an answer to.
    Request {
        /// Its identifier, which the answer repeats.
        id: RequestId,
        /// What kind.
        method: String,
        /// Its parameters.
        params: Json,
    },
    /// The answer to a call made with [`Rpc::call_ordered`], in its place among what the agent said: after
    /// everything it said before it answered, and before anything it said after.
    Answer {
        /// What the caller named the call.
        tag: u64,
        /// The answer, or why there is none.
        result: Result<Json, RpcError>,
    },
    /// The harness closed its output; nothing more will come.
    Closed {
        /// Why, as well as the adapter can tell.
        reason: String,
    },
}

fn incoming_weight(event: &Incoming) -> usize {
    std::mem::size_of::<Incoming>().saturating_add(match event {
        Incoming::Notification { method, params } => {
            method.capacity().saturating_add(flow::json_weight(params))
        }
        Incoming::Request { id, method, params } => {
            let id = match id {
                RequestId::Number(_) => 0,
                RequestId::Text(text) => text.capacity(),
            };
            id.saturating_add(method.capacity())
                .saturating_add(flow::json_weight(params))
        }
        Incoming::Answer { result, .. } => match result {
            Ok(value) => flow::json_weight(value),
            Err(error) => error.to_string().capacity(),
        },
        Incoming::Closed { reason } => reason.capacity(),
    })
}

enum Outgoing {
    Line(String, Option<futures::future::BoxFuture<'static, bool>>),
    Finish,
}

/// Who waits for an answer.
enum Waiter {
    /// A caller that awaits it.
    Call(oneshot::Sender<Result<Json, RpcError>>),
    /// The stream of what the agent says: the answer is one more item of it, under `tag`.
    Ordered { tag: u64, method: String },
}

type Pending = HashMap<i64, Waiter>;

struct Inner {
    out: mpsc::UnboundedSender<Outgoing>,
    terminal: Mutex<Vec<Incoming>>,
    close_signal: Notify,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    closed: Mutex<Option<String>>,
    tap: Option<FrameTap>,
}

impl Inner {
    fn close(&self, reason: &str) {
        let first = {
            let mut closed = self.closed.lock().unwrap_or_else(PoisonError::into_inner);
            if closed.is_some() {
                false
            } else {
                *closed = Some(reason.to_owned());
                true
            }
        };
        if first {
            let waiting: Vec<_> = self
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain()
                .collect();
            for (_, waiter) in waiting {
                match waiter {
                    Waiter::Call(waiter) => {
                        let _ = waiter.send(Err(RpcError::Closed(reason.to_owned())));
                    }
                    Waiter::Ordered { tag, .. } => {
                        self.terminal
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .push(Incoming::Answer {
                                tag,
                                result: Err(RpcError::Closed(reason.to_owned())),
                            });
                    }
                }
            }
            self.close_signal.notify_one();
        }
    }

    fn closed_reason(&self) -> Option<String> {
        self.closed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn write(&self, frame: &Json) -> Result<(), RpcError> {
        if let Some(reason) = self.closed_reason() {
            return Err(RpcError::Closed(reason));
        }
        let tap = self.tap.as_ref().map(|tap| tap(Direction::Out, frame));
        self.out
            .send(Outgoing::Line(frame.to_string(), tap))
            .map_err(|_| RpcError::Closed("the writer of the harness's input is gone".to_owned()))
    }
}

/// The adapter's end of the connection to a harness.
#[derive(Clone)]
pub struct Rpc {
    inner: Arc<Inner>,
}

impl fmt::Debug for Rpc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rpc")
            .field("closed", &self.inner.closed_reason())
            .finish()
    }
}

/// Starts the tasks that carry frames over `reader` and `writer`.
///
/// The returned receiver yields what the harness says of its own accord, in the order it said it, and ends
/// with [`Incoming::Closed`]. `tap` sees every frame of both directions.
pub fn connect<R, W>(reader: R, writer: W, tap: Option<FrameTap>) -> (Rpc, flow::Receiver<Incoming>)
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (out, lines) = mpsc::unbounded_channel();
    let (incoming_tx, incoming_rx) = flow::channel(incoming_weight);
    let inner = Arc::new(Inner {
        out,
        terminal: Mutex::new(Vec::new()),
        close_signal: Notify::new(),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicI64::new(1),
        closed: Mutex::new(None),
        tap,
    });
    tokio::spawn(write_loop(writer, lines, Arc::clone(&inner)));
    tokio::spawn(read_loop(reader, Arc::clone(&inner), incoming_tx));
    (Rpc { inner }, incoming_rx)
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut lines: mpsc::UnboundedReceiver<Outgoing>,
    inner: Arc<Inner>,
) {
    while let Some(next) = lines.recv().await {
        match next {
            Outgoing::Line(line, tap) => {
                if let Some(tap) = tap
                    && !tap.await
                {
                    inner.close("the host stopped consuming adapter events");
                    return;
                }
                let written = async {
                    writer.write_all(line.as_bytes()).await?;
                    writer.write_all(b"\n").await?;
                    writer.flush().await
                }
                .await;
                if let Err(error) = written {
                    inner.close(&format!("the harness no longer reads its input: {error}"));
                    return;
                }
            }
            Outgoing::Finish => break,
        }
    }
    let _ = writer.shutdown().await;
}

async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut Vec<u8>,
    limit: usize,
) -> io::Result<usize> {
    let mut total = 0;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(total);
        }
        if let Some(end) = available.iter().position(|byte| *byte == b'\n') {
            out.extend_from_slice(&available[..end]);
            reader.consume(end + 1);
            return Ok(total + end + 1);
        }
        let taken = available.len();
        out.extend_from_slice(available);
        reader.consume(taken);
        total += taken;
        if out.len() > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "a line of the harness is longer than any the adapter reads",
            ));
        }
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    reader: R,
    inner: Arc<Inner>,
    incoming: flow::Sender<Incoming>,
) {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    let reason = loop {
        line.clear();
        match read_frame(&mut reader, &mut line, MAX_FRAME_BYTES).await {
            Ok(0) => break "the harness closed its output".to_owned(),
            Ok(_) => {}
            Err(error) => break format!("the output of the harness could not be read: {error}"),
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let frame: Json = match serde_json::from_slice(&line) {
            Ok(frame) => frame,
            Err(error) => {
                tracing::warn!(%error, "a line of the harness is not JSON, and is skipped");
                continue;
            }
        };
        if let Some(tap) = &inner.tap
            && !tap(Direction::In, &frame).await
        {
            break "the host stopped consuming adapter events".to_owned();
        }
        if dispatch(&inner, &incoming, frame).await.is_err() {
            break "the adapter stopped consuming native events".into();
        }
    };
    inner.close(&reason);
    let terminal = std::mem::take(
        &mut *inner
            .terminal
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    );
    for event in terminal {
        if incoming.send(event).await.is_err() {
            return;
        }
    }
    let _ = incoming.send(Incoming::Closed { reason }).await;
}

async fn dispatch(
    inner: &Inner,
    incoming: &flow::Sender<Incoming>,
    mut frame: Json,
) -> Result<(), ()> {
    let id = frame.get("id").and_then(RequestId::from_json);
    let method = frame
        .get("method")
        .and_then(Json::as_str)
        .map(str::to_owned);
    match (id, method) {
        (Some(id), Some(method)) => {
            let params = frame
                .get_mut("params")
                .map(Json::take)
                .unwrap_or(Json::Null);
            incoming
                .send(Incoming::Request { id, method, params })
                .await
                .map_err(|_| ())?;
        }
        (None, Some(method)) => {
            let params = frame
                .get_mut("params")
                .map(Json::take)
                .unwrap_or(Json::Null);
            incoming
                .send(Incoming::Notification { method, params })
                .await
                .map_err(|_| ())?;
        }
        (Some(RequestId::Number(id)), None) => {
            let waiter = inner
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&id);
            let Some(waiter) = waiter else {
                tracing::debug!(id, "an answer arrived for a request nobody waits for");
                return Ok(());
            };
            let method = match &waiter {
                Waiter::Ordered { method, .. } => method.clone(),
                Waiter::Call(_) => String::new(),
            };
            let answer = if let Some(error) = frame.get_mut("error") {
                Err(RpcError::Remote {
                    method,
                    code: error.get("code").and_then(Json::as_i64).unwrap_or_default(),
                    // Some agents use a generic RPC message and put the actual refusal in data.
                    // Keep that cause ahead of incidental process diagnostics.
                    message: error
                        .pointer("/data/details")
                        .and_then(Json::as_str)
                        .filter(|detail| !detail.trim().is_empty())
                        .or_else(|| error.get("message").and_then(Json::as_str))
                        .unwrap_or_default()
                        .to_owned(),
                    data: error.get_mut("data").map(|data| Box::new(data.take())),
                })
            } else {
                Ok(frame
                    .get_mut("result")
                    .map(Json::take)
                    .unwrap_or(Json::Null))
            };
            match waiter {
                Waiter::Call(waiter) => {
                    let _ = waiter.send(answer);
                }
                Waiter::Ordered { tag, .. } => {
                    incoming
                        .send(Incoming::Answer {
                            tag,
                            result: answer,
                        })
                        .await
                        .map_err(|_| ())?;
                }
            }
        }
        _ => tracing::warn!(
            "a frame of the harness is neither a request, a notification nor an answer"
        ),
    }
    Ok(())
}

impl Rpc {
    /// Whether the connection is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.closed_reason().is_some()
    }

    /// Why the connection is gone, if it is.
    #[must_use]
    pub fn closed_reason(&self) -> Option<String> {
        self.inner.closed_reason()
    }

    /// Asks the harness something and waits for the answer.
    pub async fn call(&self, method: &str, params: Json) -> Result<Json, RpcError> {
        self.call_within(method, params, REQUEST_TIMEOUT).await
    }

    /// Asks the harness something and waits for the answer for at most `timeout`.
    pub async fn call_within(
        &self,
        method: &str,
        params: Json,
        timeout: Duration,
    ) -> Result<Json, RpcError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (waiter, answer) = oneshot::channel();
        {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(reason) = self.inner.closed_reason() {
                return Err(RpcError::Closed(reason));
            }
            pending.insert(id, Waiter::Call(waiter));
        }
        let mut frame = json!({ "jsonrpc": JSONRPC, "id": id, "method": method });
        if !params.is_null() {
            frame["params"] = params;
        }
        if let Err(error) = self.inner.write(&frame) {
            self.forget(id);
            return Err(error);
        }
        match tokio::time::timeout(timeout, answer).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(RpcError::Remote {
                code,
                message,
                data,
                ..
            }))) => Err(RpcError::Remote {
                method: method.to_owned(),
                code,
                message,
                data,
            }),
            Ok(Ok(Err(other))) => Err(other),
            Ok(Err(_)) => Err(RpcError::Closed(
                self.inner
                    .closed_reason()
                    .unwrap_or_else(|| "the answer was lost".to_owned()),
            )),
            Err(_) => {
                self.forget(id);
                Err(RpcError::Timeout {
                    method: method.to_owned(),
                    seconds: timeout.as_secs(),
                })
            }
        }
    }

    /// Asks the harness something and gives the answer to the stream of what it says, as [`Incoming::Answer`]
    /// under `tag`, however long it takes. For a request whose answer ends something that updates went before.
    pub fn call_ordered(&self, method: &str, params: Json, tag: u64) -> Result<(), RpcError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut pending = self
                .inner
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some(reason) = self.inner.closed_reason() {
                return Err(RpcError::Closed(reason));
            }
            pending.insert(
                id,
                Waiter::Ordered {
                    tag,
                    method: method.to_owned(),
                },
            );
        }
        let mut frame = json!({ "jsonrpc": JSONRPC, "id": id, "method": method });
        if !params.is_null() {
            frame["params"] = params;
        }
        if let Err(error) = self.inner.write(&frame) {
            self.forget(id);
            return Err(error);
        }
        Ok(())
    }

    /// Asks the harness something and reads the answer as `T`.
    pub async fn request<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
    ) -> Result<T, RpcError> {
        let params = serde_json::to_value(params).map_err(|error| RpcError::Shape {
            method: method.to_owned(),
            detail: error.to_string(),
        })?;
        let answer = self.call(method, params).await?;
        serde_json::from_value(answer).map_err(|error| RpcError::Shape {
            method: method.to_owned(),
            detail: error.to_string(),
        })
    }

    fn forget(&self, id: i64) {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }

    /// Tells the harness something that needs no answer.
    pub fn notify(&self, method: &str, params: Option<Json>) -> Result<(), RpcError> {
        let mut frame = json!({ "jsonrpc": JSONRPC, "method": method });
        if let Some(params) = params {
            frame["params"] = params;
        }
        self.inner.write(&frame)
    }

    /// Answers a request of the harness.
    pub fn respond(&self, id: &RequestId, result: Json) -> Result<(), RpcError> {
        self.inner
            .write(&json!({ "jsonrpc": JSONRPC, "id": id.to_json(), "result": result }))
    }

    /// Refuses a request of the harness.
    pub fn respond_error(&self, id: &RequestId, code: i64, message: &str) -> Result<(), RpcError> {
        self.inner.write(&json!({
            "jsonrpc": JSONRPC,
            "id": id.to_json(),
            "error": { "code": code, "message": message }
        }))
    }

    /// Closes the harness's input, which is how a harness is asked to end.
    pub fn finish(&self) {
        let _ = self.inner.out.send(Outgoing::Finish);
    }

    /// Writes a frame as it is, for a test that plays the harness's side of a conversation.
    pub fn send_raw(&self, frame: &Json) -> Result<(), RpcError> {
        self.inner.write(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    /// An adapter end connected to the agent end of a pipe the test plays.
    fn pair() -> (
        Rpc,
        flow::Receiver<Incoming>,
        BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let (adapter_side, agent_side) = duplex(1 << 16);
        let (adapter_read, adapter_write) = tokio::io::split(adapter_side);
        let (agent_read, agent_write) = tokio::io::split(agent_side);
        let (rpc, incoming) = connect(adapter_read, adapter_write, None);
        (rpc, incoming, BufReader::new(agent_read), agent_write)
    }

    async fn read_line(
        reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ) -> Json {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[tokio::test]
    async fn a_call_is_answered_by_the_frame_that_repeats_its_id_and_carries_the_version() {
        let (rpc, _incoming, mut agent_in, mut agent_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("session/new", json!({ "cwd": "/w" })).await }
        });
        let asked = read_line(&mut agent_in).await;
        assert_eq!(asked["method"], "session/new");
        assert_eq!(asked["params"], json!({ "cwd": "/w" }));
        assert_eq!(asked["jsonrpc"], "2.0");
        let id = asked["id"].clone();
        agent_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "jsonrpc": "2.0", "id": id, "result": { "sessionId": "s1" } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(call.await.unwrap().unwrap(), json!({ "sessionId": "s1" }));
    }

    #[tokio::test]
    async fn an_error_answer_names_what_was_asked() {
        let (rpc, _incoming, mut agent_in, mut agent_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("session/resume", json!({})).await }
        });
        let asked = read_line(&mut agent_in).await;
        agent_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "jsonrpc": "2.0", "id": asked["id"], "error": { "code": -32602, "message": "session is not resumable" } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let error = call.await.unwrap().unwrap_err();
        assert!(matches!(
            &error,
            RpcError::Remote { method, code: -32602, message, .. }
                if method == "session/resume" && message == "session is not resumable"
        ));
    }

    #[tokio::test]
    async fn an_error_keeps_the_agents_details_as_its_cause_and_preserves_its_data() {
        for details in [
            json!("the provider has no compatible protocol"),
            json!("  "),
            Json::Null,
            json!({"nested": true}),
        ] {
            let (rpc, _incoming, mut agent_in, mut agent_out) = pair();
            let call = tokio::spawn({
                let rpc = rpc.clone();
                async move { rpc.call("session/new", json!({})).await }
            });
            let asked = read_line(&mut agent_in).await;
            let data = json!({"details": details, "trace": "kept"});
            agent_out
                .write_all(
                    format!(
                        "{}\n",
                        json!({
                            "jsonrpc": "2.0", "id": asked["id"],
                            "error": {"code": -32603, "message": "Internal error", "data": data}
                        })
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let error = call.await.unwrap().unwrap_err();
            let expected = details
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Internal error");
            assert!(error.to_string().ends_with(expected));
            assert!(
                matches!(error, RpcError::Remote {method, code: -32603, message, data: Some(kept)}
                if method == "session/new" && message == expected && *kept == data)
            );
        }
    }

    #[tokio::test]
    async fn requests_and_notifications_of_the_agent_arrive_in_order_and_are_answered_by_id() {
        let (rpc, mut incoming, mut agent_in, mut agent_out) = pair();
        agent_out
            .write_all(
                format!(
                    "{}\n{}\n",
                    json!({ "jsonrpc": "2.0", "method": "session/update", "params": { "sessionId": "s" } }),
                    json!({ "jsonrpc": "2.0", "id": "p1", "method": "session/request_permission", "params": { "sessionId": "s" } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Notification { method, .. }) if method == "session/update"
        ));
        let Some(Incoming::Request { id, method, params }) = incoming.recv().await else {
            panic!("a request");
        };
        assert_eq!(method, "session/request_permission");
        assert_eq!(params["sessionId"], "s");
        rpc.respond(&id, json!({ "outcome": { "outcome": "cancelled" } }))
            .unwrap();
        assert_eq!(
            read_line(&mut agent_in).await,
            json!({ "jsonrpc": "2.0", "id": "p1", "result": { "outcome": { "outcome": "cancelled" } } })
        );
    }

    #[tokio::test]
    async fn a_line_that_is_no_json_is_skipped_and_the_conversation_goes_on() {
        let (_rpc, mut incoming, _agent_in, mut agent_out) = pair();
        agent_out
            .write_all(
                format!(
                    "plugin printed a banner\n{}\n",
                    json!({ "jsonrpc": "2.0", "method": "session/update", "params": {} })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Notification { .. })
        ));
    }

    #[tokio::test]
    async fn when_the_agent_goes_away_every_call_in_flight_fails_and_the_stream_ends() {
        let (rpc, mut incoming, mut agent_in, agent_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("session/prompt", json!({})).await }
        });
        read_line(&mut agent_in).await;
        drop(agent_out);
        drop(agent_in);
        assert!(matches!(call.await.unwrap(), Err(RpcError::Closed(_))));
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Closed { .. })
        ));
        assert!(rpc.is_closed());
        assert!(matches!(
            rpc.call("session/list", json!({})).await,
            Err(RpcError::Closed(_))
        ));
    }

    #[tokio::test]
    async fn a_call_that_is_not_answered_in_time_gives_up_and_a_late_answer_is_dropped() {
        let (rpc, _incoming, mut agent_in, mut agent_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move {
                rpc.call_within("session/list", json!({}), Duration::from_millis(50))
                    .await
            }
        });
        let asked = read_line(&mut agent_in).await;
        assert!(matches!(call.await.unwrap(), Err(RpcError::Timeout { .. })));
        agent_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "jsonrpc": "2.0", "id": asked["id"], "result": {} })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let second = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("session/list", json!({})).await }
        });
        let asked = read_line(&mut agent_in).await;
        agent_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "jsonrpc": "2.0", "id": asked["id"], "result": { "sessions": [] } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(second.await.unwrap().unwrap(), json!({ "sessions": [] }));
    }

    #[tokio::test]
    async fn a_line_longer_than_the_most_the_adapter_reads_is_an_error_and_is_not_held_whole() {
        let mut reader = BufReader::with_capacity(1024, &[b'x'; 10 * 1024][..]);
        let mut line = Vec::new();
        let error = read_frame(&mut reader, &mut line, 4 * 1024)
            .await
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(
            line.len() <= 5 * 1024,
            "stopped soon after the most: {}",
            line.len()
        );
        let mut short = BufReader::with_capacity(1024, &b"{\"a\":1}\nrest"[..]);
        let mut out = Vec::new();
        assert_eq!(read_frame(&mut short, &mut out, 4 * 1024).await.unwrap(), 8);
        assert_eq!(out, b"{\"a\":1}");
    }
}
