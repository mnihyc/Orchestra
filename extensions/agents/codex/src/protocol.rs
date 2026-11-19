//! Initialize and exchange app-server requests.
//!
//! The app-server speaks JSON-RPC in lines: one JSON object for each line of the harness's standard input and
//! output, without the `jsonrpc` member. [`connect`] starts the two tasks that read and write them over any
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
use tokio::sync::{OwnedSemaphorePermit, Semaphore, oneshot, watch};

/// How long a request waits for its answer: a little under the sixty seconds the engine allows a command, so
/// that a harness that does not answer is reported by the adapter itself before its caller gives up.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(50);

/// The default wait for `initialize`: cold startup on a busy host needs room before it reads input.
/// A provider's `initialize_timeout_secs` option replaces this default.
pub const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(120);

/// The longest line read from the harness: 128 MiB. The longest legitimate line is the history of a long
/// conversation in one answer; a line beyond this is a harness gone wrong, and reading it whole would only
/// use up the memory of the machine.
pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;

/// The error code of a request the harness does not know (JSON-RPC: method not found).
pub const METHOD_NOT_FOUND: i64 = -32601;

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
        Incoming::Closed { reason } => reason.capacity(),
    })
}

enum Outgoing {
    Line {
        line: String,
        tap: Option<futures::future::BoxFuture<'static, bool>>,
        weight: usize,
    },
}

type Pending = HashMap<i64, oneshot::Sender<Result<Json, RpcError>>>;

struct Inner {
    out: flow::Sender<Outgoing>,
    finishing: watch::Sender<bool>,
    calls: Arc<Semaphore>,
    pending: Mutex<Pending>,
    next_id: AtomicI64,
    closed: Mutex<Option<String>>,
    closed_watch: watch::Sender<Option<String>>,
    tap: Option<FrameTap>,
    native_exit: watch::Sender<bool>,
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
            self.calls.close();
            self.closed_watch.send_replace(Some(reason.to_owned()));
            let waiting: Vec<_> = self
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain()
                .collect();
            for (_, waiter) in waiting {
                let _ = waiter.send(Err(RpcError::Closed(reason.to_owned())));
            }
        }
    }

    fn closed_reason(&self) -> Option<String> {
        self.closed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    async fn write(&self, frame: Json) -> Result<(), RpcError> {
        if let Some(reason) = self.closed_reason() {
            return Err(RpcError::Closed(reason));
        }
        if *self.finishing.borrow() {
            return Err(RpcError::Closed("the harness input is closing".into()));
        }
        let weight = flow::json_weight(&frame);
        let tap = self.tap.as_ref().map(|tap| tap(Direction::Out, &frame));
        let line = frame.to_string();
        if line.len() > MAX_FRAME_BYTES {
            return Err(RpcError::Shape {
                method: "write".into(),
                detail: "the request exceeds the native frame limit".into(),
            });
        }
        let weight = weight.saturating_add(line.capacity());
        let mut closed = self.closed_watch.subscribe();
        if let Some(reason) = closed.borrow().clone() {
            return Err(RpcError::Closed(reason));
        }
        let send = self.out.send(Outgoing::Line { line, tap, weight });
        tokio::select! {
            result = send => result.map_err(|_| RpcError::Closed("the writer of the harness's input is gone".into())),
            _ = closed.changed() => Err(RpcError::Closed(self.closed_reason().unwrap_or_else(|| "the harness connection ended".into()))),
            () = tokio::time::sleep(REQUEST_TIMEOUT) => {
                self.close("the harness input queue remained full before the request deadline");
                Err(RpcError::Closed("the harness input queue remained full before the request deadline".into()))
            }
        }
    }
}

struct Waiting {
    inner: Arc<Inner>,
    id: i64,
    _slot: OwnedSemaphorePermit,
}
impl Drop for Waiting {
    fn drop(&mut self) {
        self.inner
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.id);
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
    let (out, lines) = flow::channel(|message: &Outgoing| match message {
        Outgoing::Line { weight, .. } => *weight,
    });
    let (incoming_tx, incoming_rx) = flow::channel(incoming_weight);
    let inner = Arc::new(Inner {
        out,
        finishing: watch::channel(false).0,
        calls: Arc::new(Semaphore::new(flow::QUEUE_ITEMS)),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicI64::new(1),
        closed: Mutex::new(None),
        closed_watch: watch::channel(None).0,
        tap,
        native_exit: watch::channel(false).0,
    });
    tokio::spawn(write_loop(writer, lines, Arc::clone(&inner)));
    tokio::spawn(read_loop(reader, Arc::clone(&inner), incoming_tx));
    (Rpc { inner }, incoming_rx)
}

async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut lines: flow::Receiver<Outgoing>,
    inner: Arc<Inner>,
) {
    let mut finishing = inner.finishing.subscribe();
    let mut closing = *finishing.borrow();
    if closing {
        lines.close();
    }
    loop {
        let next = tokio::select! {
            _ = finishing.changed(), if !closing => {
                closing = true;
                lines.close();
                continue;
            }
            next = lines.recv() => next,
        };
        let Some(Outgoing::Line { line, tap, .. }) = next else {
            break;
        };
        if let Some(tap) = tap
            && !tap.await
        {
            inner.close("the host stopped consuming adapter events");
            return;
        }
        let writing = async {
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await
        };
        match tokio::time::timeout(REQUEST_TIMEOUT, writing).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                inner.close(&format!("the harness no longer reads its input: {error}"));
                return;
            }
            Err(_) => {
                inner.close("the harness did not read its input before the request deadline");
                return;
            }
        }
    }
    let _ = writer.shutdown().await;
}

async fn read_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut Vec<u8>,
) -> io::Result<usize> {
    let mut total = 0;
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(total);
        }
        if let Some(end) = available.iter().position(|byte| *byte == b'\n') {
            if out.len().saturating_add(end) > MAX_FRAME_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "a line of the harness is longer than any the adapter reads",
                ));
            }
            out.extend_from_slice(&available[..end]);
            reader.consume(end + 1);
            return Ok(total + end + 1);
        }
        let taken = available.len();
        out.extend_from_slice(available);
        reader.consume(taken);
        total += taken;
        if out.len() > MAX_FRAME_BYTES {
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
    let mut native_exit = inner.native_exit.subscribe();
    let mut native_deadline = None;
    let reason = 'frames: loop {
        if native_deadline.is_none() && *native_exit.borrow() {
            native_deadline = Some(tokio::time::Instant::now() + crate::runtime::EXIT_GRACE / 2);
        }
        line.clear();
        let read = {
            // The exit watch must not cancel a partially consumed frame or reset its byte count.
            let reading = read_frame(&mut reader, &mut line);
            tokio::pin!(reading);
            loop {
                tokio::select! {
                    read = &mut reading => break read,
                    _ = native_exit.changed(), if native_deadline.is_none() => {
                        native_deadline = Some(tokio::time::Instant::now() + crate::runtime::EXIT_GRACE / 2);
                    }
                    () = async { if let Some(at) = native_deadline { tokio::time::sleep_until(at).await; } }, if native_deadline.is_some() => {
                        break 'frames inner.closed_reason().unwrap_or_else(|| "the native program ended".into());
                    }
                }
            }
        };
        match read {
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
        let delivering = tokio::time::Instant::now();
        if let Some(tap) = &inner.tap
            && !tap(Direction::In, &frame).await
        {
            break "the host stopped consuming adapter events".to_owned();
        }
        if dispatch(&inner, &incoming, frame).await.is_err() {
            break "the adapter no longer consumes native events".to_owned();
        }
        // Host delivery of an already decoded frame is not native EOF waiting.
        if let Some(deadline) = &mut native_deadline {
            *deadline += delivering.elapsed();
        }
    };
    inner.close(&reason);
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
            let answer = if let Some(error) = frame.get_mut("error") {
                Err(RpcError::Remote {
                    method: String::new(),
                    code: error.get("code").and_then(Json::as_i64).unwrap_or_default(),
                    message: error
                        .get("message")
                        .and_then(Json::as_str)
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
            let _ = waiter.send(answer);
        }
        _ => tracing::warn!(
            "a frame of the harness is neither a request, a notification nor an answer"
        ),
    }
    Ok(())
}

impl Rpc {
    /// Ends pending calls when the process exits even if a descendant retains its streams.
    pub fn process_exited(&self, reason: &str) {
        self.inner.close(reason);
        self.inner.native_exit.send_replace(true);
    }

    /// Marks a delivery failure without inventing a native process-exit fact.
    pub fn fail_connection(&self, reason: &str) {
        self.inner.close(reason);
    }

    /// Observes transport failure independently of a blocked output consumer.
    pub async fn closed(&self) {
        let mut closed = self.inner.closed_watch.subscribe();
        loop {
            if closed.borrow().is_some() {
                return;
            }
            if closed.changed().await.is_err() {
                return;
            }
        }
    }

    /// Whether the connection is gone.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.inner.closed_reason().is_some() || *self.inner.finishing.borrow()
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
        let deadline = tokio::time::Instant::now() + timeout;
        let calling = async {
            let slot = Arc::clone(&self.inner.calls)
                .acquire_owned()
                .await
                .map_err(|_| RpcError::Closed("the request admission is closed".into()))?;
            if let Some(reason) = self.inner.closed_reason() {
                return Err(RpcError::Closed(reason));
            }
            let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
            let (waiter, answer) = oneshot::channel();
            self.inner
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, waiter);
            let _waiting = Waiting {
                inner: Arc::clone(&self.inner),
                id,
                _slot: slot,
            };
            let mut frame = json!({"id":id,"method":method});
            if !params.is_null() {
                frame["params"] = params;
            }
            self.inner.write(frame).await?;
            match answer.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(RpcError::Remote {
                    code,
                    message,
                    data,
                    ..
                })) => Err(RpcError::Remote {
                    method: method.into(),
                    code,
                    message,
                    data,
                }),
                Ok(Err(error)) => Err(error),
                Err(_) => Err(RpcError::Closed(
                    self.inner
                        .closed_reason()
                        .unwrap_or_else(|| "the answer was lost".into()),
                )),
            }
        };
        tokio::time::timeout_at(deadline, calling)
            .await
            .unwrap_or_else(|_| {
                Err(RpcError::Timeout {
                    method: method.into(),
                    seconds: timeout.as_secs(),
                })
            })
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

    /// Tells the harness something that needs no answer.
    pub async fn notify(&self, method: &str, params: Option<Json>) -> Result<(), RpcError> {
        let mut frame = json!({ "method": method });
        if let Some(params) = params {
            frame["params"] = params;
        }
        self.inner.write(frame).await
    }

    /// Answers a request of the harness.
    pub async fn respond(&self, id: &RequestId, result: Json) -> Result<(), RpcError> {
        self.inner
            .write(json!({ "id": id.to_json(), "result": result }))
            .await
    }

    /// Refuses a request of the harness.
    pub async fn respond_error(
        &self,
        id: &RequestId,
        code: i64,
        message: &str,
    ) -> Result<(), RpcError> {
        self.inner
            .write(json!({
                "id": id.to_json(),
                "error": { "code": code, "message": message }
            }))
            .await
    }

    /// Closes the harness's input, which is how a harness is asked to end.
    pub fn finish(&self) {
        self.inner.finishing.send_replace(true);
    }

    /// Writes a frame as it is, for a test that plays the harness's side of a conversation.
    pub async fn send_raw(&self, frame: &Json) -> Result<(), RpcError> {
        self.inner.write(frame.clone()).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    /// An adapter end connected to the harness end of a pipe the test plays.
    fn pair() -> (
        Rpc,
        flow::Receiver<Incoming>,
        BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        tokio::io::WriteHalf<tokio::io::DuplexStream>,
    ) {
        let (adapter_side, harness_side) = duplex(1 << 16);
        let (adapter_read, adapter_write) = tokio::io::split(adapter_side);
        let (harness_read, harness_write) = tokio::io::split(harness_side);
        let (rpc, incoming) = connect(adapter_read, adapter_write, None);
        (rpc, incoming, BufReader::new(harness_read), harness_write)
    }

    async fn read_line(
        reader: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    ) -> Json {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(&line).unwrap()
    }

    #[tokio::test]
    async fn a_call_is_answered_by_the_frame_that_repeats_its_id() {
        let (rpc, _incoming, mut harness_in, mut harness_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("thread/start", json!({ "cwd": "/w" })).await }
        });
        let asked = read_line(&mut harness_in).await;
        assert_eq!(asked["method"], "thread/start");
        assert_eq!(asked["params"], json!({ "cwd": "/w" }));
        assert!(
            asked.get("jsonrpc").is_none(),
            "the app-server's lite form has no version member"
        );
        let id = asked["id"].clone();
        harness_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "id": id, "result": { "thread": { "id": "t1" } } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(
            call.await.unwrap().unwrap(),
            json!({ "thread": { "id": "t1" } })
        );
    }

    #[tokio::test]
    async fn an_error_answer_names_what_was_asked() {
        let (rpc, _incoming, mut harness_in, mut harness_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("turn/steer", json!({})).await }
        });
        let asked = read_line(&mut harness_in).await;
        harness_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "id": asked["id"], "error": { "code": -32600, "message": "no active turn to steer" } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let error = call.await.unwrap().unwrap_err();
        assert!(matches!(
            &error,
            RpcError::Remote { method, code: -32600, message, .. }
                if method == "turn/steer" && message == "no active turn to steer"
        ));
    }

    #[tokio::test]
    async fn requests_and_notifications_of_the_harness_arrive_in_order_and_are_answered_by_id() {
        let (rpc, mut incoming, mut harness_in, mut harness_out) = pair();
        harness_out
            .write_all(
                format!(
                    "{}\n{}\n",
                    json!({ "method": "turn/started", "params": { "threadId": "t" } }),
                    json!({ "id": 0, "method": "item/commandExecution/requestApproval", "params": { "itemId": "i" } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Notification { method, .. }) if method == "turn/started"
        ));
        let Some(Incoming::Request { id, method, params }) = incoming.recv().await else {
            panic!("a request");
        };
        assert_eq!(method, "item/commandExecution/requestApproval");
        assert_eq!(params["itemId"], "i");
        rpc.respond(&id, json!({ "decision": "accept" }))
            .await
            .unwrap();
        assert_eq!(
            read_line(&mut harness_in).await,
            json!({ "id": 0, "result": { "decision": "accept" } })
        );
    }

    #[tokio::test]
    async fn when_the_harness_goes_away_every_call_in_flight_fails_and_the_stream_ends() {
        let (rpc, mut incoming, mut harness_in, harness_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("turn/start", json!({})).await }
        });
        read_line(&mut harness_in).await;
        drop(harness_out);
        drop(harness_in);
        assert!(matches!(call.await.unwrap(), Err(RpcError::Closed(_))));
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Closed { .. })
        ));
        assert!(rpc.is_closed());
        assert!(matches!(
            rpc.call("account/read", json!({})).await,
            Err(RpcError::Closed(_))
        ));
    }

    #[tokio::test]
    async fn a_call_that_is_not_answered_in_time_gives_up_and_a_late_answer_is_dropped() {
        let (rpc, _incoming, mut harness_in, mut harness_out) = pair();
        let call = tokio::spawn({
            let rpc = rpc.clone();
            async move {
                rpc.call_within("model/list", json!({}), Duration::from_millis(50))
                    .await
            }
        });
        let asked = read_line(&mut harness_in).await;
        assert!(matches!(call.await.unwrap(), Err(RpcError::Timeout { .. })));
        harness_out
            .write_all(format!("{}\n", json!({ "id": asked["id"], "result": {} })).as_bytes())
            .await
            .unwrap();
        let second = tokio::spawn({
            let rpc = rpc.clone();
            async move { rpc.call("account/read", json!({})).await }
        });
        let asked = read_line(&mut harness_in).await;
        harness_out
            .write_all(
                format!(
                    "{}\n",
                    json!({ "id": asked["id"], "result": { "account": null } })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        assert_eq!(second.await.unwrap().unwrap(), json!({ "account": null }));
    }

    #[tokio::test]
    async fn stalled_native_input_bounds_a_hundred_thousand_ordered_requests() {
        use std::sync::atomic::AtomicUsize;
        const FRAMES: usize = 100_000;
        const TEXT_BYTES: usize = 256;
        let (rpc, _incoming, mut harness_in, _harness_out) = pair();
        let sent = Arc::new(AtomicUsize::new(0));
        let produced = Arc::clone(&sent);
        let (full, filled) = oneshot::channel();
        let writer = rpc.clone();
        let producer = tokio::spawn(async move {
            let mut full = Some(full);
            let text = "x".repeat(TEXT_BYTES);
            for sequence in 0..FRAMES {
                writer
                    .notify("ordered", Some(json!({"sequence":sequence,"text":text})))
                    .await
                    .unwrap();
                produced.fetch_add(1, Ordering::AcqRel);
                if writer.inner.out.usage().items == flow::QUEUE_ITEMS
                    && let Some(full) = full.take()
                {
                    let _ = full.send(());
                }
            }
        });
        filled.await.unwrap();
        tokio::task::yield_now().await;
        assert!(!producer.is_finished());
        let usage = rpc.inner.out.usage();
        assert!(usage.items <= flow::QUEUE_ITEMS);
        assert!(usage.bytes <= flow::QUEUE_BYTES);
        assert!(sent.load(Ordering::Acquire) < FRAMES);
        for sequence in 0..FRAMES {
            let frame = read_line(&mut harness_in).await;
            assert_eq!(
                frame["params"]["sequence"].as_u64(),
                Some(u64::try_from(sequence).unwrap())
            );
        }
        producer.await.unwrap();
        assert_eq!(sent.load(Ordering::Acquire), FRAMES);
        rpc.finish();
    }

    #[tokio::test]
    async fn large_native_requests_push_back_on_bytes_before_the_node_limit() {
        const FRAMES: usize = 128;
        const TEXT_BYTES: usize = 256 * 1024;
        let (rpc, _incoming, mut harness_in, _harness_out) = pair();
        let (full, filled) = oneshot::channel();
        let writer = rpc.clone();
        let producer = tokio::spawn(async move {
            let mut full = Some(full);
            let text = "x".repeat(TEXT_BYTES);
            for sequence in 0..FRAMES {
                writer
                    .notify("large", Some(json!({"sequence":sequence,"text":text})))
                    .await
                    .unwrap();
                if writer.inner.out.usage().bytes > flow::QUEUE_BYTES / 2
                    && let Some(full) = full.take()
                {
                    let _ = full.send(());
                }
            }
        });
        filled.await.unwrap();
        tokio::task::yield_now().await;
        let usage = rpc.inner.out.usage();
        assert!(usage.bytes <= flow::QUEUE_BYTES);
        assert!(usage.items < flow::QUEUE_ITEMS);
        assert!(!producer.is_finished());
        for sequence in 0..FRAMES {
            let frame = read_line(&mut harness_in).await;
            assert_eq!(
                frame["params"]["sequence"].as_u64(),
                Some(u64::try_from(sequence).unwrap())
            );
            assert_eq!(frame["params"]["text"].as_str().unwrap().len(), TEXT_BYTES);
        }
        producer.await.unwrap();
        rpc.finish();
    }

    #[tokio::test]
    async fn cancelling_a_hundred_thousand_calls_releases_every_pending_slot() {
        const CALLS: usize = 100_000;
        let (rpc, _incoming, _harness_in, _harness_out) = pair();
        for _ in 0..CALLS {
            let mut call = Box::pin(rpc.call("cancelled", json!({})));
            assert!(futures::poll!(call.as_mut()).is_pending());
            drop(call);
            assert!(rpc.inner.pending.lock().unwrap().is_empty());
            assert_eq!(rpc.inner.calls.available_permits(), flow::QUEUE_ITEMS);
        }
        assert!(rpc.inner.out.usage().items <= flow::QUEUE_ITEMS);
        assert!(rpc.inner.out.usage().bytes <= flow::QUEUE_BYTES);
        rpc.finish();
    }

    #[tokio::test(start_paused = true)]
    async fn the_request_deadline_includes_waiting_for_call_admission() {
        let (rpc, _incoming, _harness_in, _harness_out) = pair();
        let held = Arc::clone(&rpc.inner.calls)
            .acquire_many_owned(u32::try_from(flow::QUEUE_ITEMS).unwrap())
            .await
            .unwrap();
        let result = rpc
            .call_within("blocked", Json::Null, Duration::from_millis(50))
            .await;
        assert!(matches!(result, Err(RpcError::Timeout { .. })));
        assert!(rpc.inner.pending.lock().unwrap().is_empty());
        drop(held);
        assert_eq!(rpc.inner.calls.available_permits(), flow::QUEUE_ITEMS);
        rpc.finish();
    }
    #[tokio::test]
    async fn a_stalled_native_event_consumer_pushes_back_stdout_without_losing_frames() {
        use std::sync::atomic::AtomicUsize;
        const FRAMES: usize = 100_000;
        const TEXT_BYTES: usize = 256;
        const PIPE_BYTES: usize = 4096;
        let (reader, mut native) = tokio::io::duplex(PIPE_BYTES);
        let (rpc, mut incoming) = connect(reader, tokio::io::sink(), None);
        let sent = Arc::new(AtomicUsize::new(0));
        let produced = Arc::clone(&sent);
        let (full_tx, full_rx) = oneshot::channel();
        let producer = tokio::spawn(async move {
            let mut full_tx = Some(full_tx);
            let text = "x".repeat(TEXT_BYTES);
            for sequence in 0..FRAMES {
                let line = format!(
                    "{}\n",
                    json!({"method":"flood","params":{"sequence":sequence,"text":text}})
                );
                native.write_all(line.as_bytes()).await.unwrap();
                produced.fetch_add(1, Ordering::AcqRel);
                if sequence + 1 == flow::QUEUE_ITEMS {
                    full_tx.take().unwrap().send(()).unwrap();
                }
            }
        });
        full_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert!(!producer.is_finished());
        // Queue nodes, one decoded frame and the bounded OS-like pipe are the only retained frames.
        assert!(sent.load(Ordering::Acquire) <= flow::QUEUE_ITEMS + PIPE_BYTES / TEXT_BYTES + 2);
        for sequence in 0..FRAMES {
            let Some(Incoming::Notification { method, params }) = incoming.recv().await else {
                panic!("every native frame must arrive, in order");
            };
            assert_eq!(method, "flood");
            assert_eq!(
                params["sequence"].as_u64(),
                Some(u64::try_from(sequence).unwrap())
            );
            assert_eq!(params["text"].as_str().unwrap().len(), TEXT_BYTES);
        }
        producer.await.unwrap();
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Closed { .. })
        ));
        assert_eq!(sent.load(Ordering::Acquire), FRAMES);
        rpc.finish();
    }
    #[tokio::test(start_paused = true)]
    async fn a_native_exit_watch_keeps_the_prefix_of_an_in_flight_frame() {
        // One byte of pipe capacity proves the reader has consumed part of the prefix before exit.
        let (reader, mut native) = tokio::io::duplex(1);
        let (rpc, mut incoming) = connect(reader, tokio::io::sink(), None);
        native
            .write_all(b"{\"method\":\"final\",\"params\":{\"text\":\"")
            .await
            .unwrap();
        rpc.process_exited("the parent ended while its final pipe bytes were still arriving");
        tokio::task::yield_now().await;
        native.write_all(b"kept words\"}}\n").await.unwrap();
        drop(native);
        let Some(Incoming::Notification { method, params }) = incoming.recv().await else {
            panic!("the prefix read before the exit watch must still belong to its frame");
        };
        assert_eq!(method, "final");
        assert_eq!(params["text"], "kept words");
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Closed { .. })
        ));
        rpc.finish();
    }

    #[tokio::test(start_paused = true)]
    async fn buffered_native_frames_survive_exit_while_the_event_consumer_is_stalled() {
        const FRAMES: usize = flow::QUEUE_ITEMS * 2;
        const PIPE_BYTES: usize = 64 * 1024;
        let (reader, mut native) = tokio::io::duplex(PIPE_BYTES);
        let (rpc, mut incoming) = connect(reader, tokio::io::sink(), None);
        for sequence in 0..FRAMES {
            native
                .write_all(
                    format!(
                        "{}\n",
                        json!({"method":"buffered","params":{"sequence":sequence}})
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
        }
        drop(native);
        rpc.process_exited("the native program ended after writing its final events");
        tokio::time::sleep(crate::runtime::EXIT_GRACE * 2).await;
        for sequence in 0..FRAMES {
            let Some(Incoming::Notification { method, params }) = incoming.recv().await else {
                panic!("every final native event must survive host backpressure");
            };
            assert_eq!(method, "buffered");
            assert_eq!(
                params["sequence"].as_u64(),
                Some(u64::try_from(sequence).unwrap())
            );
        }
        assert!(matches!(
            incoming.recv().await,
            Some(Incoming::Closed { .. })
        ));
        rpc.finish();
    }
}
