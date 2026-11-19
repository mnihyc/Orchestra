//! Tool servers run for an environment: a stdio server as a child process whose MCP is offered as streamable HTTP on the loopback behind a token, a server reached by address relayed to, the tools filtered, and a server that crashes started again within its budget.
//!
//! The registry of [`crate::mcp`] knows which servers an environment has, where each came from and how often
//! one may crash. This module runs them. A **stdio** server is a child process in a process group of its own,
//! started in the workspace or the home as its definition says, with the connector's own variables kept from
//! it, the definition's variables and the values of its secret variables; the host speaks MCP to it over its
//! pipes and offers it, for every session that reaches it, as MCP streamable HTTP on `127.0.0.1` at a port
//! the system gave, behind a bearer token. An **http** server is one reached by address from inside the
//! environment; the host relays to it the same way.
//!
//! One server serves every session of its environment, so the host keeps no session of its own: it answers
//! `initialize` from what the server said when the host started it, answers `ping` itself, and forwards every
//! other request under an identifier of its own, so that two sessions' ids never meet. The tools the
//! definition leaves out are left out of `tools/list` and refused in `tools/call`. A call that can take long
//! is answered as a stream that carries the server's progress and a line now and then, so that a path that
//! closes a quiet exchange does not close it; a call whose client goes away is cancelled at the server.
//!
//! A server that dies is started again after a short pause, at most as often as its definition's budget in
//! the registry allows within its window; the one that exceeds it is reported failed and its address is
//! closed. A server that cannot be started at all is failed the same way, with the reason.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::body::{Body, Frame, Incoming};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_agent_contract::discovery::{
    McpAuthentication, McpDefinition, McpServerSpec, McpTransport, McpWorkingDir,
};
use orch_types::{Id, Json};
use serde_json::json;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::environment::{Report, Reports};
use crate::error::{HostError, Result};
use crate::mcp::{McpServers, Origin, Restart, tool_offered};
use crate::platform;
use crate::processes::{Processes, RootKind};
use crate::terminal::CONNECTOR_PRIVATE_VARIABLES;

/// The revision of the protocol the host speaks to a server it starts. A server that answers with another
/// revision is spoken to in that one; the host passes requests and answers through as they are.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// The path a hosted server answers at.
pub const MCP_PATH: &str = "/mcp";

/// How many random bytes make the token of a hosted server: 256 bits, as the token of a harness process.
const TOKEN_BYTES: usize = 32;

/// How many random ports of a range are tried before the range is given up on.
const PORT_TRIES: usize = 64;

/// The largest request the host reads: a tool call carries what an agent wrote into one argument, and
/// the largest model request the forwarder takes is the largest thing an agent can have said.
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

/// How much of what a server wrote to its error output is kept to explain its end: the last of it.
const STDERR_KEEP_BYTES: usize = 4096;

/// How long a server that died is left alone before it is started again: long enough for what killed it
/// (a port, a lock) to be let go, short against the ten minutes its budget counts over.
pub const DEFAULT_RESTART_PAUSE: Duration = Duration::from_secs(1);

/// How often a call that is being carried out is written a line: a path closes an exchange that is quiet
/// for ten minutes, and a client that went away is noticed by a write that fails.
pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(10);

/// How long a request waits for the server's answer: the most a call may ask of an agent's tool, an hour.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(3600);

/// How long a server is given to end on `TERM` before what it started is killed.
const STOP_GRACE: Duration = Duration::from_secs(3);

/// How much of a server's error output is read at a time: a line of a log, a few of a stack trace.
const STDERR_CHUNK_BYTES: usize = 1024;

/// How long a call in flight is given to be answered when its server is stopped or given up on: the answer
/// is the error that the server is gone, which is ready at once, so this only bounds a client that does not
/// read it.
const CONNECTION_GRACE: Duration = Duration::from_secs(2);

/// How many frames of a streamed answer wait for a client that reads slowly: progress is a trickle, and a
/// client that falls this far behind is slower than the server it asks.
const STREAM_BACKLOG: usize = 16;

/// The JSON-RPC code of a method that is not there.
const METHOD_NOT_FOUND: i64 = -32601;
/// The JSON-RPC code of parameters that are wrong: a tool that is not offered.
const INVALID_PARAMS: i64 = -32602;
/// The JSON-RPC code of a message that is no JSON.
const PARSE_ERROR: i64 = -32700;
/// The JSON-RPC code of a request that is no request.
const INVALID_REQUEST: i64 = -32600;
/// The JSON-RPC code of a failure of the host or of the server behind it.
const INTERNAL_ERROR: i64 = -32603;

/// How the host behaves, apart from the definition of a server.
#[derive(Debug, Clone)]
pub struct McpHostConfig {
    /// The operator's HTTP/TLS client and trust roots, when explicitly supplied by composition.
    pub http_client: Option<reqwest::Client>,
    /// The ports servers may listen on, on the machine itself; `None` lets the system give one.
    pub ports: Option<RangeInclusive<u16>>,
    /// How long a server that died is left alone before it is started again.
    pub restart_pause: Duration,
    /// How often a call being carried out is written a line.
    pub keep_alive: Duration,
    /// How long a request waits for an answer.
    pub request_timeout: Duration,
}

impl Default for McpHostConfig {
    fn default() -> Self {
        Self {
            http_client: None,
            ports: None,
            restart_pause: DEFAULT_RESTART_PAUSE,
            keep_alive: DEFAULT_KEEP_ALIVE,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// Where a hosted server is reached.
#[derive(Clone, PartialEq, Eq)]
pub struct McpEndpoint {
    /// The loopback address: `127.0.0.1:<port>`; the path is [`MCP_PATH`].
    pub address: SocketAddr,
    /// The bearer token a request has to carry.
    pub token: String,
}

impl std::fmt::Debug for McpEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpEndpoint")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

/// Where a server is started and what it is given besides its definition.
#[derive(Debug, Clone)]
pub struct Launch {
    /// The workspace root of the environment.
    pub workspace: PathBuf,
    /// The home of the user the environment's commands run as, where there is one.
    pub home: Option<PathBuf>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn mask_values(mut text: String, values: &[String]) -> String {
    for value in values.iter().filter(|value| !value.is_empty()) {
        text = text.replace(value, "[masked]");
    }
    text
}

fn masked_values(spec: &McpServerSpec, secrets: &BTreeMap<String, String>) -> Vec<String> {
    let mut values = secrets
        .values()
        .chain(spec.env.values())
        .filter(|value| !value.is_empty())
        .cloned()
        .collect::<Vec<_>>();
    values.sort_by_key(|value| std::cmp::Reverse(value.len()));
    values.dedup();
    values
}

/// Why a request to a server did not come back with an answer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
enum CallError {
    /// The server is gone: it died, or it was stopped, while the request waited.
    #[error("{0}")]
    Gone(String),
    /// The server did not answer in the time a request has.
    #[error("the server did not answer in time")]
    Timeout,
    /// The server could not be reached.
    #[error("{0}")]
    Unreachable(String),
}

// ---------------------------------------------------------------------------------------------------------
// A stdio server, spoken to over its pipes

/// A request that waits for its answer, and what it has registered: when it is dropped without its answer
/// (its client went away), the server is told that nobody waits for it any more.
struct Waiting {
    link: Arc<StdioLink>,
    id: u64,
    progress_key: Option<String>,
    done: bool,
}

impl Drop for Waiting {
    fn drop(&mut self) {
        lock(&self.link.pending).remove(&self.id);
        if let Some(key) = &self.progress_key {
            lock(&self.link.progress).remove(key);
        }
        if !self.done {
            self.link.send(&json!({
                "jsonrpc": "2.0", "method": "notifications/cancelled",
                "params": { "requestId": self.id, "reason": "the caller went away" }
            }));
        }
    }
}

/// The progress of one call: the token the client gave it, and where its notifications go.
type ProgressSink = (Json, mpsc::UnboundedSender<Json>);

/// A started stdio server: the pipes to it and the requests that wait on it.
struct StdioLink {
    out: mpsc::UnboundedSender<String>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Json>>>,
    progress: Mutex<HashMap<String, ProgressSink>>,
    next: AtomicU64,
    gone: CancellationToken,
    stderr: Mutex<VecDeque<u8>>,
    masked_values: Vec<String>,
    stderr_done: CancellationToken,
}

impl StdioLink {
    fn send(&self, message: &Json) {
        if let Ok(line) = serde_json::to_string(message) {
            let _ = self.out.send(line);
        }
    }

    /// What the server wrote to its error output last.
    fn stderr_tail(&self) -> String {
        let kept = lock(&self.stderr);
        let bytes: Vec<u8> = kept.iter().copied().collect();
        let masked = mask_values(
            String::from_utf8_lossy(&bytes).trim().to_owned(),
            &self.masked_values,
        );
        let mut start = masked.len().saturating_sub(STDERR_KEEP_BYTES);
        while !masked.is_char_boundary(start) {
            start += 1;
        }
        masked[start..].to_owned()
    }

    /// Sends a request under an identifier of the host's own and waits for the answer: the whole message
    /// the server answered with. A call that has a progress token gets a token of the host's own, and the
    /// notifications that carry it come back under the client's.
    async fn request(
        self: &Arc<Self>,
        method: &str,
        mut params: Json,
        progress: Option<ProgressSink>,
        patience: Duration,
    ) -> std::result::Result<Json, CallError> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (answer, answered) = oneshot::channel();
        lock(&self.pending).insert(id, answer);
        let mut waiting = Waiting {
            link: Arc::clone(self),
            id,
            progress_key: None,
            done: false,
        };
        if let Some(sink) = progress {
            let key = format!("orch-{id}");
            if let Some(meta) = params
                .get_mut("_meta")
                .and_then(serde_json::Value::as_object_mut)
            {
                meta.insert("progressToken".to_owned(), Json::String(key.clone()));
            }
            lock(&self.progress).insert(key.clone(), sink);
            waiting.progress_key = Some(key);
        }
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        let outcome = tokio::select! {
            answered = answered => answered.map_err(|_| CallError::Gone(self.explain_end())),
            () = self.gone.cancelled() => Err(CallError::Gone(self.explain_end())),
            () = tokio::time::sleep(patience) => Err(CallError::Timeout),
        };
        waiting.done = outcome.is_ok();
        outcome
    }

    fn explain_end(&self) -> String {
        let tail = self.stderr_tail();
        if tail.is_empty() {
            "the tool server ended".to_owned()
        } else {
            format!("the tool server ended: {tail}")
        }
    }
}

/// Reads what a server writes, a message to a line, and gives each answer to the request that waits for it.
async fn read_messages(link: Arc<StdioLink>, stdout: tokio::process::ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(message) = serde_json::from_str::<Json>(&line) else {
            continue;
        };
        let method = message.get("method").and_then(Json::as_str);
        let id = message.get("id").cloned();
        match (method, id) {
            (None, Some(id)) => {
                let waiting = id.as_u64().and_then(|id| lock(&link.pending).remove(&id));
                if let Some(answer) = waiting {
                    let _ = answer.send(message);
                }
            }
            (Some(_), Some(id)) => {
                // The host declared no capability a server could ask of it: nothing is asked of a client.
                link.send(&json!({
                    "jsonrpc": "2.0", "id": id,
                    "error": { "code": METHOD_NOT_FOUND, "message": "this host has no such method" }
                }));
            }
            (Some("notifications/progress"), None) => {
                let key = message
                    .pointer("/params/progressToken")
                    .and_then(Json::as_str)
                    .map(str::to_owned);
                let target = key.and_then(|key| lock(&link.progress).get(&key).cloned());
                if let Some((token, sink)) = target {
                    let mut told = message.clone();
                    if let Some(params) = told.get_mut("params").and_then(Json::as_object_mut) {
                        params.insert("progressToken".to_owned(), token);
                    }
                    let _ = sink.send(told);
                }
            }
            _ => {}
        }
    }
    link.gone.cancel();
    lock(&link.pending).clear();
}

/// Writes the lines the host sends to a server.
async fn write_messages(
    mut lines: mpsc::UnboundedReceiver<String>,
    mut stdin: tokio::process::ChildStdin,
) {
    while let Some(line) = lines.recv().await {
        let written = async {
            stdin.write_all(line.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await
        }
        .await;
        if written.is_err() {
            return;
        }
    }
}

/// Keeps the last of what a server writes to its error output.
async fn keep_stderr(link: Arc<StdioLink>, mut stderr: tokio::process::ChildStderr) {
    let mut chunk = [0u8; STDERR_CHUNK_BYTES];
    while let Ok(count) = stderr.read(&mut chunk).await {
        if count == 0 {
            break;
        }
        let mut kept = lock(&link.stderr);
        kept.extend(&chunk[..count]);
        while kept.len()
            > STDERR_KEEP_BYTES.saturating_add(link.masked_values.first().map_or(0, String::len))
        {
            kept.pop_front();
        }
    }
    link.stderr_done.cancel();
}

// ---------------------------------------------------------------------------------------------------------
// A server reached by address

/// A server reached over HTTP or authenticated HTTPS from inside the environment.
struct HttpLink {
    target: String,
    client: reqwest::Client,
    authorization: Option<reqwest::header::HeaderValue>,
    session: Mutex<Option<String>>,
}

/// Authentication never comes from public variables, and invalid values never appear in a failure reason.
fn upstream_authorization(
    spec: &McpServerSpec,
    secrets: &BTreeMap<String, String>,
) -> Result<Option<reqwest::header::HeaderValue>> {
    let Some(McpAuthentication::Bearer { secret }) = &spec.authentication else {
        return Ok(None);
    };
    if spec.transport != McpTransport::Http || !spec.secret_env.contains(secret) {
        return Err(HostError::Unsupported(
            "HTTP authentication must name a declared secret".to_owned(),
        ));
    }
    let token = secrets
        .get(secret)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            HostError::Unsupported(
                "the tool server's authentication token has not been set".to_owned(),
            )
        })?;
    let mut value = reqwest::header::HeaderValue::from_bytes(format!("Bearer {token}").as_bytes())
        .map_err(|_| {
            HostError::Unsupported(
                "the tool server's authentication token must be a single-line value".to_owned(),
            )
        })?;
    value.set_sensitive(true);
    Ok(Some(value))
}

impl HttpLink {
    /// Posts one message to the server and reads its answer: a JSON body, or the first message of a stream
    /// that has the answer to `id`.
    async fn post(
        &self,
        message: &Json,
        id: Option<u64>,
    ) -> std::result::Result<Option<Json>, CallError> {
        let unreachable = |what: String| CallError::Unreachable(what);
        let body = serde_json::to_vec(message).map_err(|e| unreachable(e.to_string()))?;
        let mut request = self
            .client
            .post(&self.target)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(value) = &self.authorization {
            request = request.header(reqwest::header::AUTHORIZATION, value.clone());
        }
        if let Some(session) = lock(&self.session).clone() {
            request = request.header("mcp-session-id", session);
        }
        let mut response = request.body(body).send().await.map_err(|_| {
            unreachable(
                "the HTTP tool server could not be reached with the configured TLS trust"
                    .to_owned(),
            )
        })?;
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *lock(&self.session) = Some(session.to_owned());
        }
        let status = response.status();
        let streamed = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        if status == StatusCode::ACCEPTED {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(unreachable(format!(
                "the HTTP tool server answered {status}"
            )));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| unreachable("the HTTP tool server's reply was interrupted".to_owned()))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_REQUEST_BYTES {
                return Err(unreachable(
                    "the HTTP tool server's reply exceeds the message limit".to_owned(),
                ));
            }
            bytes.extend_from_slice(&chunk);
            if streamed {
                let text = String::from_utf8_lossy(&bytes).replace("\r\n", "\n");
                if let Some(message) = text
                    .split("\n\n")
                    .filter_map(|event| {
                        let data = event
                            .lines()
                            .filter_map(|line| line.strip_prefix("data:"))
                            .map(str::trim)
                            .collect::<Vec<_>>()
                            .join("\n");
                        serde_json::from_str::<Json>(&data).ok()
                    })
                    .find(|message| id.is_none() || message.get("id").and_then(Json::as_u64) == id)
                {
                    return Ok(Some(message));
                }
            }
        }
        if bytes.is_empty() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let found = if streamed {
            text.split("\n\n")
                .flat_map(|event| {
                    event
                        .lines()
                        .filter_map(|line| line.strip_prefix("data:"))
                        .map(|data| data.trim().to_owned())
                        .collect::<Vec<_>>()
                })
                .filter_map(|data| serde_json::from_str::<Json>(&data).ok())
                .find(|message| id.is_none() || message.get("id").and_then(Json::as_u64) == id)
        } else {
            serde_json::from_str::<Json>(&text).ok()
        };
        Ok(found)
    }
}

// ---------------------------------------------------------------------------------------------------------
// What a hosted server is behind its address

enum Backend {
    Stdio(Arc<StdioLink>),
    Http(Arc<HttpLink>),
}

impl Clone for Backend {
    fn clone(&self) -> Self {
        match self {
            Self::Stdio(link) => Self::Stdio(Arc::clone(link)),
            Self::Http(link) => Self::Http(Arc::clone(link)),
        }
    }
}

impl Backend {
    async fn request(
        &self,
        method: &str,
        params: Json,
        progress: Option<ProgressSink>,
        patience: Duration,
        ids: &AtomicU64,
    ) -> std::result::Result<Json, CallError> {
        match self {
            Self::Stdio(link) => link.request(method, params, progress, patience).await,
            Self::Http(link) => {
                let id = ids.fetch_add(1, Ordering::SeqCst);
                let message =
                    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
                match tokio::time::timeout(patience, link.post(&message, Some(id))).await {
                    Ok(Ok(Some(answer))) => Ok(answer),
                    Ok(Ok(None)) => Err(CallError::Unreachable(
                        "the server answered nothing".to_owned(),
                    )),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(CallError::Timeout),
                }
            }
        }
    }
}

/// What the listener and the supervisor of one hosted server share.
struct Shared {
    name: String,
    token: String,
    spec: McpServerSpec,
    /// The server as it is now; none while it is being started again or after it was given up on.
    backend: Mutex<Option<Backend>>,
    /// What the server said of itself when the host started it.
    initialize: Mutex<Json>,
    config: McpHostConfig,
    ids: AtomicU64,
    pid: AtomicU32,
    /// Fires when the server is stopped or given up on: a call in flight is over then, and is answered with
    /// the error that says so.
    closing: CancellationToken,
}

struct Hosted {
    shared: Arc<Shared>,
    address: SocketAddr,
    stop: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

#[derive(Clone)]
struct SavedLaunch {
    definition: McpDefinition,
    launch: Launch,
}

/// The tool servers of one environment, run.
pub struct McpHost {
    registry: Arc<McpServers>,
    processes: Processes,
    reports: Reports,
    environment_id: Id,
    config: McpHostConfig,
    hosted: tokio::sync::Mutex<HashMap<String, Arc<Hosted>>>,
    definitions: tokio::sync::Mutex<HashMap<String, SavedLaunch>>,
}

impl std::fmt::Debug for McpHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpHost").finish_non_exhaustive()
    }
}

/// Whether a program is found: as a path that names a file, or in a directory of `PATH`.
fn command_exists(program: &str) -> bool {
    let direct = Path::new(program);
    if direct.components().count() > 1 {
        return direct.is_file();
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|directory| directory.join(program).is_file())
    })
}

fn random_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    rand::fill(&mut bytes[..]);
    hex::encode(bytes)
}

/// Binds a listener on the machine itself: at a port of `ports` if one is given, else one the system
/// gives.
async fn bind(ports: Option<&RangeInclusive<u16>>) -> Result<TcpListener> {
    if let Some(range) = ports {
        let (first, last) = (u32::from(*range.start()), u32::from(*range.end()));
        let span = last.saturating_sub(first) + 1;
        for _ in 0..PORT_TRIES {
            let mut random = [0u8; 4];
            rand::fill(&mut random[..]);
            let port = first + u32::from_le_bytes(random) % span;
            let Ok(port) = u16::try_from(port) else {
                continue;
            };
            if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
                return Ok(listener);
            }
        }
    }
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|e| HostError::io("binding the address of a tool server", e))
}

impl McpHost {
    /// The host of the environment `environment_id`, over its registry and its process table; what happens
    /// to a server on its own account is said through `reports`.
    #[must_use]
    pub fn new(
        registry: Arc<McpServers>,
        processes: Processes,
        reports: Reports,
        environment_id: Id,
        config: McpHostConfig,
    ) -> Self {
        Self {
            registry,
            processes,
            reports,
            environment_id,
            config,
            hosted: tokio::sync::Mutex::new(HashMap::new()),
            definitions: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Starts the server `name` from its definition, and says where it is reached. A server that runs under
    /// that name is stopped first: starting again is a restart on purpose, and its crashes are counted anew.
    pub async fn start(
        &self,
        name: &str,
        origin: Origin,
        definition: McpDefinition,
        launch: &Launch,
    ) -> Result<McpEndpoint> {
        self.start_identified(name, origin, definition, launch, None)
            .await
    }

    /// Records stable membership before startup so failed or slow starts keep the same identity.
    pub async fn start_identified(
        &self,
        name: &str,
        origin: Origin,
        definition: McpDefinition,
        launch: &Launch,
        server_id: Option<Id>,
    ) -> Result<McpEndpoint> {
        definition.check().map_err(|faults| {
            HostError::Unsupported(format!("that definition is not complete: {faults:?}"))
        })?;
        let previous = self.hosted.lock().await.remove(name);
        if let Some(previous) = previous {
            previous.stop().await;
            self.processes_end(&previous).await;
            let _ = self.registry.stopped(name);
        }
        self.registry.add(name, origin, definition.spec.clone())?;
        if server_id.is_some() {
            self.registry.identify(name, server_id)?;
        }
        let values = definition
            .secrets
            .iter()
            .map(|(key, value)| (key.clone(), value.expose().to_owned()))
            .collect();
        let masks = masked_values(&definition.spec, &values);
        self.definitions.lock().await.insert(
            name.to_owned(),
            SavedLaunch {
                definition: definition.clone(),
                launch: launch.clone(),
            },
        );
        match self.launch(name, definition, launch).await {
            Ok(hosted) => {
                let endpoint = McpEndpoint {
                    address: hosted.address,
                    token: hosted.shared.token.clone(),
                };
                self.registry.started(name)?;
                self.hosted.lock().await.insert(name.to_owned(), hosted);
                Ok(endpoint)
            }
            Err(error) => {
                let reason = mask_values(error.to_string(), &masks);
                self.registry.failed(name, &reason);
                Err(HostError::Unsupported(reason))
            }
        }
    }

    async fn processes_end(&self, hosted: &Hosted) {
        let pid = hosted.shared.pid.load(Ordering::SeqCst);
        if pid != 0 {
            let _ = self.processes.stop_tree(pid, STOP_GRACE).await;
        }
    }

    /// Restarts just the existing authorized definition. None lets a new connector restore it from start inputs.
    pub async fn restart(&self, name: &str) -> Result<Option<McpEndpoint>> {
        let Some(saved) = self.definitions.lock().await.get(name).cloned() else {
            return Ok(None);
        };
        self.stop(name).await?;
        self.registry.restart(name)?;
        let values = saved
            .definition
            .secrets
            .iter()
            .map(|(key, value)| (key.clone(), value.expose().to_owned()))
            .collect();
        let masks = masked_values(&saved.definition.spec, &values);
        match self.launch(name, saved.definition, &saved.launch).await {
            Ok(hosted) => {
                self.registry.started(name)?;
                let endpoint = McpEndpoint {
                    address: hosted.address,
                    token: hosted.shared.token.clone(),
                };
                self.hosted.lock().await.insert(name.to_owned(), hosted);
                Ok(Some(endpoint))
            }
            Err(error) => {
                let reason = mask_values(error.to_string(), &masks);
                self.registry.failed(name, &reason);
                Err(HostError::Unsupported(reason))
            }
        }
    }

    async fn launch(
        &self,
        name: &str,
        definition: McpDefinition,
        launch: &Launch,
    ) -> Result<Arc<Hosted>> {
        let spec = definition.spec.clone();
        let secrets: BTreeMap<String, String> = definition
            .secrets
            .iter()
            .map(|(name, value)| (name.clone(), value.expose().to_owned()))
            .collect();
        if let Some(missing) = spec
            .requires
            .commands
            .iter()
            .find(|command| !command_exists(command))
        {
            return Err(HostError::Unsupported(format!(
                "{missing} is missing from this environment"
            )));
        }
        let startup = Duration::from_secs(u64::from(spec.startup_timeout_secs));
        let stop = CancellationToken::new();
        let shared = Arc::new(Shared {
            name: name.to_owned(),
            token: random_token(),
            spec: spec.clone(),
            backend: Mutex::new(None),
            initialize: Mutex::new(Json::Null),
            config: self.config.clone(),
            ids: AtomicU64::new(1),
            pid: AtomicU32::new(0),
            closing: stop.clone(),
        });
        let mut tasks = Vec::new();
        let started = match spec.transport {
            McpTransport::Stdio => {
                let spawned =
                    spawn_stdio(&self.processes, &spec, &secrets, launch, startup, &shared).await?;
                tasks.extend(spawned.tasks);
                Some((spawned.link, spawned.child))
            }
            McpTransport::Http => {
                let target = spec.url.clone().unwrap_or_default();
                let uri = reqwest::Url::parse(&target).map_err(|_| {
                    HostError::Unsupported(
                        "the tool server needs an HTTP or HTTPS address".to_owned(),
                    )
                })?;
                if !matches!(uri.scheme(), "http" | "https")
                    || !uri.username().is_empty()
                    || uri.password().is_some()
                {
                    return Err(HostError::Unsupported("the tool server needs an HTTP or HTTPS address without embedded credentials".to_owned()));
                }
                let client = match &self.config.http_client {
                    Some(client) => client.clone(),
                    None => reqwest::Client::builder()
                        .redirect(reqwest::redirect::Policy::none())
                        .timeout(self.config.request_timeout)
                        .build()
                        .map_err(|_| {
                            HostError::Unsupported(
                                "the tool server's TLS client could not be configured".to_owned(),
                            )
                        })?,
                };
                let link = Arc::new(HttpLink {
                    target,
                    client,
                    authorization: upstream_authorization(&spec, &secrets)?,
                    session: Mutex::new(None),
                });
                let reason =
                    |e: CallError| HostError::Unsupported(format!("the server did not start: {e}"));
                let hello = initialize_message(0);
                let answer = tokio::time::timeout(startup, link.post(&hello, Some(0)))
                    .await
                    .map_err(|_| {
                        HostError::Timeout("the server did not answer in time".to_owned())
                    })?
                    .map_err(reason)?;
                let init = answer
                    .and_then(|message| message.get("result").cloned())
                    .ok_or_else(|| {
                        HostError::Unsupported("the server answered no initialize".to_owned())
                    })?;
                *lock(&shared.initialize) = init;
                let _ = link
                    .post(
                        &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
                        None,
                    )
                    .await;
                *lock(&shared.backend) = Some(Backend::Http(link));
                None
            }
        };
        let listener = bind(self.config.ports.as_ref()).await?;
        let address = listener
            .local_addr()
            .map_err(|e| HostError::io("reading the address of a tool server", e))?;
        tasks.push(tokio::spawn(accept(
            listener,
            Arc::clone(&shared),
            stop.clone(),
        )));
        if let Some((link, child)) = started {
            tasks.push(tokio::spawn(supervise(
                SuperviseContext {
                    shared: Arc::clone(&shared),
                    registry: Arc::clone(&self.registry),
                    processes: self.processes.clone(),
                    reports: self.reports.clone(),
                    environment_id: self.environment_id,
                    spec,
                    secrets,
                    launch: launch.clone(),
                    startup,
                    stop: stop.clone(),
                },
                link,
                child,
            )));
        }
        Ok(Arc::new(Hosted {
            shared,
            address,
            stop,
            tasks: Mutex::new(tasks),
        }))
    }

    /// Where the server `name` is reached, while it runs.
    pub async fn endpoint(&self, name: &str) -> Option<McpEndpoint> {
        let hosted = self.hosted.lock().await.get(name).cloned()?;
        lock(&hosted.shared.backend).is_some().then(|| McpEndpoint {
            address: hosted.address,
            token: hosted.shared.token.clone(),
        })
    }

    /// Stops the server `name`, and what it started.
    pub async fn stop(&self, name: &str) -> Result<()> {
        let hosted = self.hosted.lock().await.remove(name);
        if let Some(hosted) = hosted {
            hosted.stop().await;
            self.processes_end(&hosted).await;
        }
        self.registry.stopped(name)
    }

    /// Stops every server, as when the environment is detached.
    pub async fn stop_all(&self) {
        let names: Vec<String> = self.hosted.lock().await.keys().cloned().collect();
        for name in names {
            let _ = self.stop(&name).await;
        }
    }
}

impl Hosted {
    async fn stop(&self) {
        self.stop.cancel();
        let tasks: Vec<JoinHandle<()>> = lock(&self.tasks).drain(..).collect();
        for task in tasks {
            task.abort();
        }
        *lock(&self.shared.backend) = None;
    }
}

fn initialize_message(id: u64) -> Json {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION, "capabilities": {},
            "clientInfo": { "name": "orch-mcp-host", "version": env!("CARGO_PKG_VERSION") }
        }
    })
}

// ---------------------------------------------------------------------------------------------------------
// Starting a stdio server

struct Spawned {
    link: Arc<StdioLink>,
    child: Child,
    tasks: Vec<JoinHandle<()>>,
}

/// What a server inherits of the connector's environment: all of it but what is the connector's own.
fn inherited() -> Vec<(String, String)> {
    std::env::vars()
        .filter(|(name, _)| !CONNECTOR_PRIVATE_VARIABLES.contains(&name.as_str()))
        .collect()
}

async fn spawn_stdio(
    processes: &Processes,
    spec: &McpServerSpec,
    secrets: &BTreeMap<String, String>,
    launch: &Launch,
    startup: Duration,
    shared: &Arc<Shared>,
) -> Result<Spawned> {
    let argv = spec.command.clone().unwrap_or_default();
    let Some((program, arguments)) = argv.split_first() else {
        return Err(HostError::Unsupported(
            "a stdio server names the program to run".to_owned(),
        ));
    };
    let directory = match spec.working_dir {
        McpWorkingDir::Workspace => launch.workspace.clone(),
        McpWorkingDir::Home => launch
            .home
            .clone()
            .unwrap_or_else(|| launch.workspace.clone()),
    };
    let mut command = Command::from(processes.command(program));
    command
        .args(arguments)
        .env_clear()
        .envs(inherited())
        .envs(&spec.env)
        .envs(secrets)
        .current_dir(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    platform::prepare_group(command.as_std_mut());
    let mut child = command
        .spawn()
        .map_err(|e| HostError::Unsupported(format!("{program} could not be started: {e}")))?;
    if let Some(pid) = child.id() {
        shared.pid.store(pid, Ordering::SeqCst);
        if let Err(error) = processes.register_root(pid, RootKind::Server) {
            tracing::warn!(pid, %error, "a tool server just started could not be noted");
        }
    }
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(HostError::Internal(
            "a tool server was started without its pipes".to_owned(),
        ));
    };
    let (out, lines) = mpsc::unbounded_channel();
    let link = Arc::new(StdioLink {
        out,
        pending: Mutex::new(HashMap::new()),
        progress: Mutex::new(HashMap::new()),
        next: AtomicU64::new(1),
        gone: CancellationToken::new(),
        stderr: Mutex::new(VecDeque::new()),
        masked_values: masked_values(spec, secrets),
        stderr_done: CancellationToken::new(),
    });
    let tasks = vec![
        tokio::spawn(write_messages(lines, stdin)),
        tokio::spawn(read_messages(Arc::clone(&link), stdout)),
        tokio::spawn(keep_stderr(Arc::clone(&link), stderr)),
    ];
    let mut spawned = Spawned { link, child, tasks };
    match handshake(&spawned.link, startup).await {
        Ok(init) => {
            *lock(&shared.initialize) = init;
            *lock(&shared.backend) = Some(Backend::Stdio(Arc::clone(&spawned.link)));
            Ok(spawned)
        }
        Err(error) => {
            for task in &spawned.tasks {
                task.abort();
            }
            if let Some(pid) = spawned.child.id() {
                let _ = processes.stop_tree(pid, STOP_GRACE).await;
            }
            let _ = spawned.child.wait().await;
            Err(error)
        }
    }
}

/// Makes the server say what it is: the host's first request, within the time its definition gives.
async fn handshake(link: &Arc<StdioLink>, startup: Duration) -> Result<Json> {
    let hello = initialize_message(0);
    let params = hello.get("params").cloned().unwrap_or(Json::Null);
    let answer = link
        .request("initialize", params, None, startup)
        .await
        .map_err(|error| match error {
            CallError::Timeout => HostError::Timeout(format!(
                "the server did not say what it is within {} seconds",
                startup.as_secs()
            )),
            other => HostError::Unsupported(format!("the server did not start: {other}")),
        })?;
    if let Some(error) = answer.get("error") {
        return Err(HostError::Unsupported(format!(
            "the server refused to start: {error}"
        )));
    }
    let result = answer.get("result").cloned().unwrap_or(Json::Null);
    link.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    Ok(result)
}

// ---------------------------------------------------------------------------------------------------------
// Keeping a server going

struct SuperviseContext {
    shared: Arc<Shared>,
    registry: Arc<McpServers>,
    processes: Processes,
    reports: Reports,
    environment_id: Id,
    spec: McpServerSpec,
    secrets: BTreeMap<String, String>,
    launch: Launch,
    startup: Duration,
    stop: CancellationToken,
}

/// Waits for the server to end and starts it again while its budget allows; when it does not, the server is
/// reported failed and its address is closed.
async fn supervise(context: SuperviseContext, mut link: Arc<StdioLink>, mut child: Child) {
    loop {
        let status = tokio::select! {
            () = context.stop.cancelled() => return,
            status = child.wait() => status,
        };
        if context.stop.is_cancelled() {
            return;
        }
        let ended = status.map_or_else(|e| e.to_string(), |status| status.to_string());
        let _ = tokio::time::timeout(Duration::from_secs(1), link.stderr_done.cancelled()).await;
        link.gone.cancel();
        let mut reason = format!("the server ended ({ended})");
        let tail = link.stderr_tail();
        if !tail.is_empty() {
            reason.push_str(": ");
            reason.push_str(&tail);
        }
        *lock(&context.shared.backend) = None;
        let name = context.shared.name.clone();
        loop {
            match context.registry.crashed(&name, &reason) {
                Ok(Restart::Again) => {}
                Ok(Restart::GiveUp) | Err(_) => {
                    let failed = format!("it crashed too often: {reason}");
                    tracing::warn!(server = %name, %failed, "a tool server is given up on");
                    context
                        .reports
                        .say(
                            context.environment_id,
                            Report::McpFailed {
                                name,
                                reason: failed,
                            },
                        )
                        .await;
                    context.stop.cancel();
                    return;
                }
            }
            tokio::select! {
                () = context.stop.cancelled() => return,
                () = tokio::time::sleep(context.shared.config.restart_pause) => {}
            }
            match spawn_stdio(
                &context.processes,
                &context.spec,
                &context.secrets,
                &context.launch,
                context.startup,
                &context.shared,
            )
            .await
            {
                Ok(spawned) => {
                    // The pipes of the new process are served by tasks that end with it.
                    link = spawned.link;
                    child = spawned.child;
                    let _ = context.registry.started(&name);
                    break;
                }
                Err(error) => {
                    reason = format!("it could not be started again: {error}");
                }
            }
        }
        context
            .reports
            .say(
                context.environment_id,
                Report::McpRestarted {
                    name: context.shared.name.clone(),
                },
            )
            .await;
    }
}

// ---------------------------------------------------------------------------------------------------------
// The address

/// What a request is answered with.
type Reply = BoxBody<Bytes, Infallible>;

fn reply(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Reply> {
    let mut response = Response::new(Full::new(Bytes::from(body)).boxed());
    *response.status_mut() = status;
    if let Ok(value) = content_type.parse() {
        response
            .headers_mut()
            .insert(hyper::header::CONTENT_TYPE, value);
    }
    response
}

fn plain(status: StatusCode, message: &str) -> Response<Reply> {
    reply(
        status,
        "text/plain; charset=utf-8",
        message.as_bytes().to_vec(),
    )
}

fn rpc_error(id: &Json, code: i64, message: &str) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn json_reply(status: StatusCode, body: &Json) -> Response<Reply> {
    reply(
        status,
        "application/json",
        serde_json::to_vec(body).unwrap_or_default(),
    )
}

/// The body of a response that is written as it goes: the frames the task that answers sends.
struct Streamed {
    frames: mpsc::Receiver<Bytes>,
}

impl Body for Streamed {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Bytes>, Infallible>>> {
        self.frames
            .poll_recv(cx)
            .map(|frame| frame.map(|bytes| Ok(Frame::data(bytes))))
    }
}

async fn accept(listener: TcpListener, shared: Arc<Shared>, stop: CancellationToken) {
    loop {
        let accepted = tokio::select! {
            () = stop.cancelled() => return,
            accepted = listener.accept() => accepted,
        };
        let Ok((stream, _)) = accepted else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let shared = Arc::clone(&shared);
        let stop = stop.clone();
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let shared = Arc::clone(&shared);
                async move { Ok::<_, Infallible>(handle(&shared, request).await) }
            });
            let connection = hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service);
            tokio::pin!(connection);
            tokio::select! {
                _ = connection.as_mut() => {}
                () = stop.cancelled() => {
                    // The calls in flight are answered, with the error that the server is gone, before the
                    // connection ends: an address that is closed under a client says nothing of why.
                    connection.as_mut().graceful_shutdown();
                    let _ = tokio::time::timeout(CONNECTION_GRACE, connection.as_mut()).await;
                }
            }
        });
    }
}

fn bearer(request: &Request<Incoming>) -> &str {
    request
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
}

/// Whether the client takes a stream of events as the answer.
fn takes_a_stream(request: &Request<Incoming>) -> bool {
    request
        .headers()
        .get_all(hyper::header::ACCEPT)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|accepted| accepted.contains("text/event-stream"))
}

async fn handle(shared: &Arc<Shared>, request: Request<Incoming>) -> Response<Reply> {
    if !bool::from(bearer(&request).as_bytes().ct_eq(shared.token.as_bytes())) {
        let mut response = plain(StatusCode::UNAUTHORIZED, "a bearer token is required");
        response.headers_mut().insert(
            hyper::header::WWW_AUTHENTICATE,
            hyper::header::HeaderValue::from_static("Bearer"),
        );
        return response;
    }
    if request.uri().path() != MCP_PATH {
        return plain(StatusCode::NOT_FOUND, "no such address");
    }
    match *request.method() {
        Method::POST => {}
        Method::DELETE => return plain(StatusCode::OK, "there is no session to end"),
        _ => {
            let mut response = plain(
                StatusCode::METHOD_NOT_ALLOWED,
                "this server is asked with POST",
            );
            response.headers_mut().insert(
                hyper::header::ALLOW,
                hyper::header::HeaderValue::from_static("POST, DELETE"),
            );
            return response;
        }
    }
    let streams = takes_a_stream(&request);
    let body = match Limited::new(request.into_body(), MAX_REQUEST_BYTES)
        .collect()
        .await
    {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return plain(StatusCode::PAYLOAD_TOO_LARGE, "the request is too large"),
    };
    let message: Json = match serde_json::from_slice(&body) {
        Ok(message) => message,
        Err(_) => {
            return json_reply(
                StatusCode::BAD_REQUEST,
                &rpc_error(&Json::Null, PARSE_ERROR, "the request is no JSON"),
            );
        }
    };
    if let Json::Array(batch) = message {
        let mut answers = Vec::new();
        for message in batch {
            if let Answer::Now(answer) = answer(shared, message, false).await {
                answers.push(answer);
            }
        }
        return if answers.is_empty() {
            plain(StatusCode::ACCEPTED, "")
        } else {
            json_reply(StatusCode::OK, &Json::Array(answers))
        };
    }
    match answer(shared, message, streams).await {
        Answer::None => plain(StatusCode::ACCEPTED, ""),
        Answer::Now(answer) => json_reply(StatusCode::OK, &answer),
        Answer::Stream(frames) => {
            let mut response = Response::new(Streamed { frames }.boxed());
            let headers = response.headers_mut();
            headers.insert(
                hyper::header::CONTENT_TYPE,
                hyper::header::HeaderValue::from_static("text/event-stream"),
            );
            headers.insert(
                hyper::header::CACHE_CONTROL,
                hyper::header::HeaderValue::from_static("no-cache"),
            );
            response
        }
    }
}

enum Answer {
    /// Nothing is answered: a notification, or the answer to nothing that was asked.
    None,
    /// The answer, whole.
    Now(Json),
    /// The answer is written as it comes.
    Stream(mpsc::Receiver<Bytes>),
}

/// The line of a stream that carries one message.
fn event(message: &Json) -> Bytes {
    Bytes::from(format!(
        "event: message\ndata: {}\n\n",
        serde_json::to_string(message).unwrap_or_default()
    ))
}

async fn answer(shared: &Arc<Shared>, message: Json, streams: bool) -> Answer {
    let id = message.get("id").cloned();
    let method = message
        .get("method")
        .and_then(Json::as_str)
        .map(str::to_owned);
    let params = message.get("params").cloned().unwrap_or(Json::Null);
    let (Some(method), Some(id)) = (method, id) else {
        // A notification, or the client's answer to something the host never asked: nothing to say.
        return if message.get("method").is_some()
            || message.get("result").is_some()
            || message.get("error").is_some()
        {
            Answer::None
        } else {
            Answer::Now(rpc_error(
                &Json::Null,
                INVALID_REQUEST,
                "that is no JSON-RPC message",
            ))
        };
    };
    match method.as_str() {
        "initialize" => Answer::Now(json!({
            "jsonrpc": "2.0", "id": id, "result": lock(&shared.initialize).clone()
        })),
        "ping" => Answer::Now(json!({ "jsonrpc": "2.0", "id": id, "result": {} })),
        "tools/call" => {
            let tool = params
                .get("name")
                .and_then(Json::as_str)
                .unwrap_or_default();
            if !tool_offered(&shared.spec, tool) {
                return Answer::Now(rpc_error(
                    &id,
                    INVALID_PARAMS,
                    &format!("unknown tool: {tool}"),
                ));
            }
            if streams {
                Answer::Stream(stream_call(shared, id, params))
            } else {
                let answered = forward(shared, "tools/call", params, None).await;
                Answer::Now(finish(&id, answered, &shared.spec, "tools/call"))
            }
        }
        other => {
            let answered = forward(shared, other, params, None).await;
            Answer::Now(finish(&id, answered, &shared.spec, other))
        }
    }
}

async fn forward(
    shared: &Arc<Shared>,
    method: &str,
    params: Json,
    progress: Option<ProgressSink>,
) -> std::result::Result<Json, CallError> {
    let backend = lock(&shared.backend).clone();
    let Some(backend) = backend else {
        return Err(CallError::Gone("the tool server is not running".to_owned()));
    };
    tokio::select! {
        answered = backend.request(
            method,
            params,
            progress,
            shared.config.request_timeout,
            &shared.ids,
        ) => answered,
        () = shared.closing.cancelled() => {
            Err(CallError::Gone("the tool server was stopped".to_owned()))
        }
    }
}

/// The answer to the client's request: the server's answer under the client's identifier, with the tools
/// the definition leaves out taken from a list.
fn finish(
    id: &Json,
    answered: std::result::Result<Json, CallError>,
    spec: &McpServerSpec,
    method: &str,
) -> Json {
    match answered {
        Ok(mut message) => {
            message["id"] = id.clone();
            if method == "tools/list"
                && let Some(tools) = message
                    .pointer_mut("/result/tools")
                    .and_then(Json::as_array_mut)
            {
                tools.retain(|tool| {
                    tool.get("name")
                        .and_then(Json::as_str)
                        .is_some_and(|name| tool_offered(spec, name))
                });
            }
            message
        }
        Err(error) => rpc_error(id, INTERNAL_ERROR, &error.to_string()),
    }
}

/// Carries out a call and writes what comes of it to a stream: the progress the server reports for it, a
/// line now and then while it runs, and its answer last.
fn stream_call(shared: &Arc<Shared>, id: Json, params: Json) -> mpsc::Receiver<Bytes> {
    let (frames, receiver) = mpsc::channel::<Bytes>(STREAM_BACKLOG);
    let shared = Arc::clone(shared);
    tokio::spawn(async move {
        if frames
            .send(Bytes::from_static(b": ready\n\n"))
            .await
            .is_err()
        {
            return;
        }
        let (notes, mut progress) = mpsc::unbounded_channel::<Json>();
        let sink = params
            .pointer("/_meta/progressToken")
            .cloned()
            .map(|token| (token, notes));
        let call = forward(&shared, "tools/call", params, sink);
        tokio::pin!(call);
        let mut ticker = tokio::time::interval(shared.config.keep_alive);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker.tick().await;
        let answered = loop {
            tokio::select! {
                answered = &mut call => break answered,
                Some(note) = progress.recv() => {
                    if frames.send(event(&note)).await.is_err() {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if frames.send(Bytes::from_static(b": still working\n\n")).await.is_err() {
                        // The client went away: the call is dropped, and the server is told.
                        return;
                    }
                }
            }
        };
        let message = finish(&id, answered, &shared.spec, "tools/call");
        let _ = frames.send(event(&message)).await;
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::environment::Reports;
    use crate::mcp::McpGates;
    use orch_types::ManualClock;
    use tokio::net::TcpStream;

    #[tokio::test]
    async fn an_http_servers_secret_authenticates_every_protocol_request_without_becoming_public_configuration()
     {
        let spec: McpServerSpec =
            serde_json::from_value(json!({"transport":"http","url":"http://example.test/mcp",
            "authentication":{"kind":"bearer","secret":"TOKEN"},"secret_env":["TOKEN"]}))
            .unwrap();
        let secrets = BTreeMap::from([("TOKEN".to_owned(), "proof-token-value".to_owned())]);
        let authorization = upstream_authorization(&spec, &secrets).unwrap();
        assert!(!format!("{authorization:?}").contains("proof-token-value"));
        assert!(
            !serde_json::to_string(&spec)
                .unwrap()
                .contains("proof-token-value")
        );
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let count = Arc::new(AtomicU64::new(0));
        let seen = Arc::clone(&count);
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                let seen = Arc::clone(&seen);
                async move {
                    assert_eq!(
                        request
                            .headers()
                            .get("authorization")
                            .and_then(|value| value.to_str().ok()),
                        Some("Bearer proof-token-value")
                    );
                    seen.fetch_add(1, Ordering::SeqCst);
                    let bytes = request.into_body().collect().await.unwrap().to_bytes();
                    let message: Json = serde_json::from_slice(&bytes).unwrap();
                    let reply = json!({"jsonrpc":"2.0","id":message["id"],"result":{"content":[{"type":"text","text":"authorized tool result"}]}});
                    let mut response =
                        Response::new(http_body_util::Full::new(Bytes::from(reply.to_string())));
                    response.headers_mut().insert(
                        hyper::header::CONTENT_TYPE,
                        hyper::header::HeaderValue::from_static("application/json"),
                    );
                    if message.get("id").is_none() {
                        *response.status_mut() = StatusCode::NO_CONTENT;
                        *response.body_mut() = http_body_util::Full::new(Bytes::new());
                    }
                    Ok::<_, Infallible>(response)
                }
            });
            hyper::server::conn::http1::Builder::new()
                .serve_connection(TokioIo::new(socket), service)
                .await
                .unwrap();
        });
        let link = HttpLink {
            target: format!("http://{address}/mcp"),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            authorization,
            session: Mutex::new(None),
        };
        link.post(&initialize_message(1), Some(1)).await.unwrap();
        link.post(
            &json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            None,
        )
        .await
        .unwrap();
        let result = link
            .post(
                &json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search"}}),
                Some(2),
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result["result"]["content"][0]["text"],
            "authorized tool result"
        );
        assert_eq!(count.load(Ordering::SeqCst), 3);
        server.abort();
        for token in [None, Some("hidden-value\r\ninvalid-header")] {
            let values = token
                .map(|value| BTreeMap::from([("TOKEN".to_owned(), value.to_owned())]))
                .unwrap_or_default();
            let reason = upstream_authorization(&spec, &values)
                .unwrap_err()
                .to_string();
            assert!(!reason.contains("hidden-value") && !reason.contains("proof-token-value"));
        }
    }

    #[tokio::test]
    async fn an_https_tool_server_uses_declared_trust_and_a_bad_certificate_discloses_no_secret() {
        let rcgen::CertifiedKey { cert, signing_key } =
            rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).unwrap();
        let key = rustls::pki_types::PrivateKeyDer::Pkcs8(
            rustls::pki_types::PrivatePkcs8KeyDer::from(signing_key.serialize_der()),
        );
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server));
        let listener = bind(Some(&(41000..=41999))).await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    let Ok(stream) = acceptor.accept(stream).await else {
                        return;
                    };
                    let service = service_fn(|request: Request<Incoming>| async move {
                        let body = request.into_body().collect().await.unwrap().to_bytes();
                        let request: Json = serde_json::from_slice(&body).unwrap();
                        let reply =
                            json!({"jsonrpc":"2.0","id":request["id"],"result":{"ok":true}});
                        Ok::<_, Infallible>(
                            Response::builder()
                                .header("content-type", "application/json")
                                .body(Full::new(Bytes::from(reply.to_string())))
                                .unwrap(),
                        )
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        let target = format!(
            "https://localhost:{}/mcp?key=secret-in-address",
            address.port()
        );
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .tls_certs_merge([reqwest::Certificate::from_der(cert.der()).unwrap()])
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let trusted = HttpLink {
            target: target.clone(),
            client,
            authorization: None,
            session: Mutex::new(None),
        };
        let answer = trusted
            .post(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}), Some(1))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(answer["result"]["ok"], true);
        let untrusted = HttpLink {
            target,
            authorization: None,
            client: reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .unwrap(),
            session: Mutex::new(None),
        };
        let refused = untrusted
            .post(&json!({"jsonrpc":"2.0","id":2,"method":"ping"}), Some(2))
            .await
            .unwrap_err()
            .to_string();
        assert!(refused.contains("TLS trust"));
        assert!(!refused.contains("secret-in-address") && !refused.contains("?key="));
        server_task.abort();
    }

    /// The program the tests run as a tool server: this very test binary, asked to run the one test below as
    /// a server. It is no test when it is run by itself.
    const SERVER_FLAG: &str = "ORCH_TEST_MCP_SERVER";

    /// The variable that names the file the stand-in writes when a call to `hang` reaches it.
    const HANG_MARK: &str = "ORCH_TEST_MCP_HANG_MARK";

    /// A tool server of the tests: it answers `initialize`, lists three tools, echoes, reports its secret
    /// variable, and ends itself on request.
    #[test]
    fn stand_in_for_a_tool_server() {
        if std::env::var(SERVER_FLAG).is_err() {
            return;
        }
        use std::io::{BufRead, Write};
        let stdin = std::io::stdin();
        let mut out = std::io::stdout();
        // The test harness has begun a line ("test … ... ") that the first message would end up on.
        let _ = writeln!(out);
        let _ = out.flush();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { return };
            let Ok(message) = serde_json::from_str::<Json>(&line) else {
                continue;
            };
            let (Some(method), id) = (
                message.get("method").and_then(Json::as_str),
                message.get("id").cloned(),
            ) else {
                continue;
            };
            let result = match method {
                "initialize" => json!({
                    "protocolVersion": PROTOCOL_VERSION, "capabilities": { "tools": {} },
                    "serverInfo": { "name": "stand-in", "version": "1" }
                }),
                "tools/list" => json!({ "tools": [
                    { "name": "echo", "description": "says what it is told", "inputSchema": { "type": "object" } },
                    { "name": "secret", "description": "tells a variable", "inputSchema": { "type": "object" } },
                    { "name": "crash", "description": "ends the server", "inputSchema": { "type": "object" } },
                    { "name": "hang", "description": "never answers", "inputSchema": { "type": "object" } }
                ]}),
                "tools/call" => {
                    let tool = message
                        .pointer("/params/name")
                        .and_then(Json::as_str)
                        .unwrap_or("");
                    match tool {
                        "crash" => {
                            eprintln!(
                                "before crash: {}",
                                std::env::var("STAND_IN_SECRET").unwrap_or_default()
                            );
                            std::process::exit(7);
                        }
                        "hang" => {
                            // Reached, and never answered: the mark says the call got here.
                            if let Ok(mark) = std::env::var(HANG_MARK) {
                                let _ = std::fs::write(mark, "reached");
                            }
                            continue;
                        }
                        "secret" => json!({ "content": [{ "type": "text",
                            "text": std::env::var("STAND_IN_SECRET").unwrap_or_default() }] }),
                        _ => json!({ "content": [{ "type": "text",
                            "text": message.pointer("/params/arguments/text").and_then(Json::as_str).unwrap_or("").to_owned() }] }),
                    }
                }
                _ => continue,
            };
            if let Some(id) = id {
                let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                let _ = writeln!(out, "{reply}");
                let _ = out.flush();
            }
        }
    }

    fn stand_in(
        budget: u32,
    ) -> (
        tempfile::TempDir,
        McpHost,
        Arc<McpServers>,
        Launch,
        McpDefinition,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let clock = ManualClock::new(chrono::Utc::now());
        let registry = Arc::new(McpServers::new(
            Arc::new(clock.clone()),
            dir.path().to_path_buf(),
            budget,
            Duration::from_secs(600),
            McpGates {
                import: true,
                add: true,
            },
        ));
        let (reports, _queue) = Reports::channel(16);
        std::mem::forget(_queue);
        let host = McpHost::new(
            Arc::clone(&registry),
            Processes::new(crate::processes::ScopeKind::Started, Arc::new(clock)),
            reports,
            orch_types::new_id(),
            McpHostConfig {
                ports: Some(41000..=41999),
                restart_pause: Duration::from_millis(50),
                ..McpHostConfig::default()
            },
        );
        let definition: McpDefinition = serde_json::from_value(json!({
            "spec": {
                "transport": "stdio",
                "command": [std::env::current_exe().unwrap(), "--exact", "mcp_host::tests::stand_in_for_a_tool_server", "--nocapture", "--test-threads=1"],
                "env": { SERVER_FLAG: "1" }, "secret_env": ["STAND_IN_SECRET"],
                "tools_deny": ["crash", "hang"], "startup_timeout_secs": 20
            },
            "secrets": { "STAND_IN_SECRET": "s3cret-value" }
        }))
        .unwrap();
        let launch = Launch {
            workspace: dir.path().to_path_buf(),
            home: None,
        };
        (dir, host, registry, launch, definition)
    }

    async fn call(endpoint: &McpEndpoint, body: &Json) -> (StatusCode, Json) {
        let stream = TcpStream::connect(endpoint.address).await.unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let request = Request::builder()
            .method(Method::POST)
            .uri(MCP_PATH)
            .header("host", endpoint.address.to_string())
            .header("authorization", format!("Bearer {}", endpoint.token))
            .header("content-type", "application/json")
            .header("accept", "application/json")
            .body(Full::new(Bytes::from(serde_json::to_vec(body).unwrap())))
            .unwrap();
        let response = sender.send_request(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Json::Null))
    }

    #[tokio::test]
    async fn a_started_server_is_reached_behind_its_token_with_the_tools_its_definition_offers() {
        let (_dir, host, registry, launch, definition) = stand_in(3);
        let endpoint = host
            .start("files", Origin::Preset, definition, &launch)
            .await
            .unwrap();
        assert_eq!(endpoint.address.ip(), Ipv4Addr::LOCALHOST);
        let (status, hello) = call(
            &endpoint,
            &json!({ "jsonrpc": "2.0", "id": "first", "method": "initialize", "params": {} }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(hello["id"], "first");
        assert_eq!(hello["result"]["serverInfo"]["name"], "stand-in");
        let (_, listed) = call(
            &endpoint,
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await;
        let names: Vec<&str> = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            ["echo", "secret"],
            "the tool the definition denies is left out"
        );
        let (_, refused) = call(
            &endpoint,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "crash" } }),
        )
        .await;
        assert_eq!(refused["error"]["code"], INVALID_PARAMS);
        let (_, echoed) = call(
            &endpoint,
            &json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call",
                     "params": { "name": "echo", "arguments": { "text": "hello" } } }),
        )
        .await;
        assert_eq!(echoed["result"]["content"][0]["text"], "hello");
        assert_eq!(echoed["id"], 4);
        let (_, secret) = call(
            &endpoint,
            &json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": { "name": "secret" } }),
        )
        .await;
        assert_eq!(
            secret["result"]["content"][0]["text"], "s3cret-value",
            "the server is given the value of its secret variable"
        );
        // Without the token nothing is answered.
        let anonymous = McpEndpoint {
            token: "wrong".to_owned(),
            ..endpoint.clone()
        };
        let (status, _) = call(
            &anonymous,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" }),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        host.stop("files").await.unwrap();
        assert!(
            registry
                .get("files")
                .is_some_and(|server| matches!(server.state, crate::mcp::ServerState::Stopped))
        );
        assert!(host.endpoint("files").await.is_none());
    }

    #[tokio::test]
    async fn a_server_that_dies_is_started_again_within_its_budget_and_then_given_up_on() {
        // These bound an absent transition under machine load, independently of restart/startup policy.
        const OBSERVATION_LIMIT: Duration = Duration::from_secs(300);
        const OBSERVATION_POLL: Duration = Duration::from_millis(20);
        let (_dir, host, registry, launch, definition) = stand_in(2);
        let mut spec = definition;
        spec.spec.tools_deny = None;
        let stable = host
            .start("steady", Origin::Preset, spec.clone(), &launch)
            .await
            .unwrap();
        let stable_pid = host.hosted.lock().await["steady"]
            .shared
            .pid
            .load(Ordering::SeqCst);
        let endpoint = host
            .start("flaky", Origin::Preset, spec, &launch)
            .await
            .unwrap();
        let crash = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "crash" } });
        let echo = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                           "params": { "name": "echo", "arguments": { "text": "again" } } });
        // Two deaths are two restarts, and the server answers again each time.
        for _ in 0..2 {
            let (_, ended) = call(&endpoint, &crash).await;
            assert!(
                ended.get("error").is_some(),
                "the call in flight is failed: {ended}"
            );
            let answered = tokio::time::timeout(OBSERVATION_LIMIT, async {
                loop {
                    let (_, tried) = call(&endpoint, &echo).await;
                    if tried.get("result").is_some() {
                        break tried;
                    }
                    tokio::time::sleep(OBSERVATION_POLL).await;
                }
            })
            .await
            .expect("the restarted server answers");
            assert_eq!(answered["result"]["content"][0]["text"], "again");
        }
        // The third death exceeds the budget: the server is failed, and its address is closed.
        let (_, ended) = call(&endpoint, &crash).await;
        assert!(ended.get("error").is_some());
        tokio::time::timeout(OBSERVATION_LIMIT, async {
            loop {
                if matches!(
                    registry.get("flaky").map(|server| server.state),
                    Some(crate::mcp::ServerState::Failed { .. })
                ) {
                    break;
                }
                tokio::time::sleep(OBSERVATION_POLL).await;
            }
        })
        .await
        .expect("the server is reported failed after its restart budget");
        match registry.get("flaky").unwrap().state {
            crate::mcp::ServerState::Failed { reason } => {
                assert!(reason.contains("crashed too often"), "{reason}");
                assert!(
                    reason.contains('7')
                        && reason.contains("before crash")
                        && reason.contains("[masked]"),
                    "{reason}"
                );
                assert!(
                    !reason.contains("s3cret-value"),
                    "the failure never reports a secret"
                );
            }
            other => panic!("{other:?}"),
        }
        tokio::time::timeout(OBSERVATION_LIMIT, async {
            while TcpStream::connect(endpoint.address).await.is_ok() {
                tokio::time::sleep(OBSERVATION_POLL).await;
            }
        })
        .await
        .expect("the address of a server that was given up on closes");
        // Restart is allowed for this already-authorized server even after new additions are disabled.
        registry.set_gates(McpGates {
            add: false,
            import: false,
        });
        let revived = host.restart("flaky").await.unwrap().unwrap();
        assert!(matches!(
            registry.get("flaky").unwrap().state,
            crate::mcp::ServerState::Running { .. }
        ));
        assert_eq!(host.endpoint("steady").await.unwrap(), stable);
        assert_eq!(
            host.hosted.lock().await["steady"]
                .shared
                .pid
                .load(Ordering::SeqCst),
            stable_pid
        );
        let (_, answer) = call(&revived, &echo).await;
        assert_eq!(answer["result"]["content"][0]["text"], "again");
        let _ = call(&revived, &crash).await;
        tokio::time::timeout(OBSERVATION_LIMIT, async {
            loop {
                if call(&revived, &echo).await.1.get("result").is_some() {
                    break;
                }
                tokio::time::sleep(OBSERVATION_POLL).await;
            }
        })
        .await
        .expect(
            "Restart cleared only the failed server's budget, so it can recover from another exit",
        );
        assert_eq!(
            host.hosted.lock().await["steady"]
                .shared
                .pid
                .load(Ordering::SeqCst),
            stable_pid
        );
    }

    #[tokio::test]
    async fn a_call_in_flight_when_its_server_is_stopped_is_answered_with_the_error_and_not_cut_off()
     {
        let (dir, host, _registry, launch, mut definition) = stand_in(3);
        definition.spec.tools_deny = None;
        let mark = dir.path().join("hang-reached");
        definition
            .spec
            .env
            .insert(HANG_MARK.to_owned(), mark.display().to_string());
        let endpoint = host
            .start("hung", Origin::Preset, definition, &launch)
            .await
            .unwrap();
        let hang = json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call",
                           "params": { "name": "hang", "arguments": {} } });
        let waiting = tokio::spawn(async move { call(&endpoint, &hang).await });
        // Until the server has the call, there is nothing in flight to answer.
        tokio::time::timeout(Duration::from_secs(20), async {
            while !mark.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the call never reached the server");
        host.stop("hung").await.unwrap();
        let (status, answer) = tokio::time::timeout(Duration::from_secs(10), waiting)
            .await
            .expect("the call was never answered")
            .expect("the client of the call failed: the connection was cut");
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["id"], 5);
        assert!(answer.get("error").is_some(), "{answer}");
    }

    #[tokio::test]
    async fn a_program_that_cannot_be_started_is_a_failed_server_with_the_reason() {
        let (_dir, host, registry, launch, mut definition) = stand_in(3);
        definition.spec.command = Some(vec!["/nonexistent/orch-test-tool-server".to_owned()]);
        let error = host
            .start("absent", Origin::Preset, definition, &launch)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("could not be started"),
            "{error}"
        );
        assert!(matches!(
            registry.get("absent").unwrap().state,
            crate::mcp::ServerState::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn a_command_the_definition_requires_and_the_environment_lacks_is_said() {
        let (_dir, host, _registry, launch, mut definition) = stand_in(3);
        definition.spec.requires.commands = vec!["orch-test-command-that-is-nowhere".to_owned()];
        let error = host
            .start("needs", Origin::Preset, definition, &launch)
            .await
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("is missing from this environment"),
            "{error}"
        );
    }
}
