//! Executors and tool servers reached through tunnels from the machine where the harness runs: an HTTP exchange forwarded to the executor's or the server's address, the messages of Codex's exec server carried both ways.
//!
//! A harness that runs apart from its tools reaches them through its connector's listener, which opens a
//! tunnel with the ticket the system signed for the session; the tunnel ends here. What it carries depends
//! on what it ends at:
//!
//! * **`mcp/exec`**, Orch's own executor, and **`mcp/<name>`**, a tool server of the environment: one HTTP
//!   exchange to each tunnel. The exchange is forwarded to the address the executor or the server was
//!   started at, on the machine itself, with the token that only this connector holds; whatever
//!   `Authorization` the other end sent is not passed on. For Orch's executor the exchange says which
//!   session it is in a header ([`SESSION_HEADER`]). What the ticket states (the thread, the tool set) is
//!   what counts; the directory, the mounts and the sandbox mode are checked here against the environment
//!   that is attached and lowered to the ceiling of this machine's policy, so that a connector that is
//!   not this machine's cannot widen what the tools of a session reach.
//! * **`exec/<kind>`**, an executor that carries a protocol of its harness's own (Codex's exec server): the
//!   tunnel carries its WebSocket messages, each framed in the stream as the kind, the length and the
//!   message. A WebSocket is opened to the executor's address, with its token, and the messages go both
//!   ways until either end ends. An executor that offers its tools as an MCP server is served as
//!   **`mcp/exec`** when it is named this way.
//!
//! A tunnel to a tool server that was given up on is refused with the reason, so that whoever opened it can
//! say why the tool is not there. The executor that has stopped answering is started again once for the
//! tunnel that finds it so.

use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Component, Path};
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::{Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{HeaderMap, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_envhost::environment::EnvHost;
use orch_envhost::executors::MCP_SERVER_NAME;
use orch_envhost::mcp_host::MCP_PATH;
use orch_executor_contract::descriptor::ExecutorProtocol;
use orch_link::machine::MachineLink;
use orch_link::tunnels::channel::{IncomingTunnel, Tunnel};
use orch_link_contract::commands::SandboxMode;
use orch_link_contract::tunnels::{ExecMessage, ExecOpcode, OpenErrorCode, Service, TicketClaims};
use orch_types::Id;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;

use crate::loopback::{Body, text};
use crate::roles::Services;

/// The header in which an exchange with Orch's own executor says which session it is in.
pub const SESSION_HEADER: &str = "x-orch-session";

/// The header in which the connector that relays an exchange says which environment its tools act in.
pub const ENVIRONMENT_HEADER: &str = "x-orch-environment";

/// The path an executor of a program's own takes its WebSocket at when its manifest says none: the root.
const EXEC_SERVER_PATH: &str = "/";

/// The largest message of an executor that is carried: a file's content or a command's output in one piece.
const EXEC_MESSAGE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// How many bytes are read from a tunnel at a time while its messages are being framed.
const TUNNEL_READ_BYTES: usize = 64 * 1024;

/// The most a request to a tool server may be: what an agent can write into one call.
const REQUEST_MAX_BYTES: usize = 32 * 1024 * 1024;

/// The headers of a request that are passed on to the executor or the server: nothing that says who the
/// other end is or how its connection was made.
const REQUEST_HEADERS: [&str; 7] = [
    "content-type",
    "content-length",
    "accept",
    "mcp-protocol-version",
    "mcp-session-id",
    "last-event-id",
    "user-agent",
];

/// The headers of an answer that are passed back.
const RESPONSE_HEADERS: [&str; 6] = [
    "content-type",
    "content-length",
    "cache-control",
    "mcp-session-id",
    "allow",
    "retry-after",
];

// ---------------------------------------------------------------------------------------------------------
// The session an exchange with Orch's own executor is in

/// Why what an exchange says of its session is not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRefusal {
    /// The exchange does not say it, or says it in a form that is not read.
    Malformed(String),
    /// It says what the environment, the ticket or the machine's policy does not allow.
    Forbidden(String),
}

impl SessionRefusal {
    /// The answer to the exchange: the status and the sentence.
    #[must_use]
    pub fn answer(&self) -> Response<Body> {
        match self {
            Self::Malformed(why) => text(StatusCode::BAD_REQUEST, why),
            Self::Forbidden(why) => text(StatusCode::FORBIDDEN, why),
        }
    }
}

/// A further place the tools of a session reach, as the executor reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecMount {
    /// The mount's name.
    pub name: String,
    /// Where it is, as an absolute path.
    pub path: String,
    /// Whether nothing may write there.
    pub read_only: bool,
}

/// The session an exchange with Orch's own executor belongs to, in the form the executor reads it: the
/// thread, the tool set, the directory, the sandbox mode and the mounts. The executor's own type is in its
/// package, which a connector does not depend on; a test with the executor decodes what is written here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecScope {
    /// The thread's identifier.
    pub thread_id: String,
    /// The tool set: `plain`, `claude-code` or `custom`.
    pub tool_set: String,
    /// The thread's directory, as an absolute path.
    pub directory: String,
    /// Where the agent starts inside it.
    #[serde(default)]
    pub cwd: Option<String>,
    /// The mode the turn runs in: `read_only`, `workspace` or `open`.
    pub sandbox_mode: SandboxMode,
    /// The mounts that are attached.
    #[serde(default)]
    pub mounts: Vec<ExecMount>,
    /// How many times the environment was made anew when the session was opened: an exchange of a session of an
    /// earlier making of the environment is refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<i64>,
}

impl ExecScope {
    /// The scope as the header carries it: JSON in unpadded base64 of the URL-safe alphabet.
    #[must_use]
    pub fn encode(&self) -> String {
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(self).unwrap_or_default())
    }

    /// The scope a header carries.
    pub fn decode(header: &str) -> Result<Self, String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(header.trim())
            .map_err(|_| "the session header is not base64".to_owned())?;
        serde_json::from_slice(&bytes)
            .map_err(|e| format!("the session header is not a session: {e}"))
    }
}

/// Whether `inner` lies inside `outer`, by their components alone, and climbs nowhere.
fn lies_inside(inner: &str, outer: &str) -> bool {
    let climbs = Path::new(inner)
        .components()
        .any(|part| matches!(part, Component::ParentDir));
    !climbs && Path::new(inner).starts_with(outer)
}

// ---------------------------------------------------------------------------------------------------------
// Where a tunnel ends

/// An executor or a tool server that runs here, and how it is reached.
#[derive(Debug, Clone)]
pub struct Target {
    /// The environment it runs in.
    pub environment_id: Id,
    /// The executor kind or the server's name.
    pub name: String,
    /// Where it listens, on the machine itself.
    pub address: SocketAddr,
    /// The path it answers at.
    pub path: String,
    /// What a request has to present.
    pub token: Option<String>,
    /// Whether it is an executor that takes the session in a header.
    pub takes_session: bool,
}

/// An address an executor was started at, taken apart: where it listens and the path it answers at.
fn parse_address(address: &str, default_path: &str) -> Option<(SocketAddr, String)> {
    let rest = address.split_once("://").map_or(address, |(_, rest)| rest);
    let (authority, path) = rest.find('/').map_or((rest, ""), |at| rest.split_at(at));
    let (host, port) = authority.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    let ip: Ipv4Addr = match host {
        "127.0.0.1" | "localhost" => Ipv4Addr::LOCALHOST,
        _ => return None,
    };
    let path = if path.is_empty() { default_path } else { path };
    Some((SocketAddr::from((ip, port)), path.to_owned()))
}

/// Carries what the tools of sessions are reached through, on the machine they run on: the executors and
/// the tool servers of its environments (when it plays the role), and the tunnels to those that run on
/// another.
pub struct ToolRelay {
    host: Option<EnvHost>,
    services: Arc<Services>,
    link: MachineLink,
}

impl std::fmt::Debug for ToolRelay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolRelay").finish_non_exhaustive()
    }
}

impl ToolRelay {
    /// A relay over the environments of `host`, if the machine plays the environment role.
    #[must_use]
    pub fn new(services: &Arc<Services>, link: &MachineLink, host: Option<EnvHost>) -> Arc<Self> {
        Arc::new(Self {
            host,
            services: Arc::clone(services),
            link: link.clone(),
        })
    }

    /// Where the tools of a session are, while the session is bound.
    #[must_use]
    pub fn reach_of(&self, session_key: &str) -> Option<crate::loopback::Reach> {
        self.services.loopback.reach(session_key)
    }

    /// The machine's link, for tunnels to executors and servers elsewhere.
    #[must_use]
    pub fn link(&self) -> &MachineLink {
        &self.link
    }

    /// The environment host, if the machine plays the environment role.
    #[must_use]
    pub fn host(&self) -> Option<&EnvHost> {
        self.host.as_ref()
    }

    /// Whether the environment is attached here.
    #[must_use]
    pub fn has_environment(&self, environment_id: Id) -> bool {
        self.host
            .as_ref()
            .is_some_and(|host| host.environment(environment_id).is_ok())
    }

    /// Whether the attached environment is the generation admitted to the session.
    #[must_use]
    pub fn environment_matches(&self, environment: Id, generation: i64) -> bool {
        self.host
            .as_ref()
            .and_then(|host| host.environment(environment).ok())
            .is_some_and(|attached| attached.generation() == generation)
    }

    fn ticket_environment(&self, claims: &TicketClaims) -> Option<(Id, i64)> {
        let environment = claims.tool_environment_id?;
        let generation = claims.tool_environment_generation?;
        (generation >= 0 && self.environment_matches(environment, generation))
            .then_some((environment, generation))
    }

    /// The running executors that speak a protocol of their harness's own and are called `kind`.
    pub async fn exec_targets(&self, kind: &str) -> Vec<Target> {
        let Some(host) = &self.host else {
            return Vec::new();
        };
        // The path an executor takes its connection at is its manifest's.
        let default_path = orch_agent_programs::Programs::builtin()
            .executor(kind)
            .and_then(|executor| executor.endpoint.as_ref())
            .map_or(EXEC_SERVER_PATH, |endpoint| endpoint.path.as_str());
        host.running_executors()
            .await
            .into_iter()
            .filter(|(_, running, protocol, _)| {
                running == kind && *protocol == ExecutorProtocol::Harness
            })
            .filter_map(|(environment_id, name, _, endpoint)| {
                let (address, path) = parse_address(&endpoint.address, default_path)?;
                Some(Target {
                    environment_id,
                    name,
                    address,
                    path,
                    token: endpoint.token.map(|token| token.expose().to_owned()),
                    takes_session: false,
                })
            })
            .collect()
    }

    /// Whether an executor called `kind` runs here and offers its tools as an MCP server.
    pub async fn offers_mcp(&self, kind: &str, environment: Id) -> bool {
        let Some(host) = &self.host else {
            return false;
        };
        host.running_executors()
            .await
            .into_iter()
            .any(|(id, running, protocol, _)| {
                id == environment && running == kind && protocol == ExecutorProtocol::Mcp
            })
    }

    /// The running servers an MCP tunnel to `server` can end at: Orch's own executor for `exec`, the tool
    /// server of that name otherwise.
    pub async fn mcp_targets(&self, server: &str) -> Vec<Target> {
        let Some(host) = &self.host else {
            return Vec::new();
        };
        if server == MCP_SERVER_NAME {
            return host
                .running_executors()
                .await
                .into_iter()
                .filter(|(_, _, protocol, _)| *protocol == ExecutorProtocol::Mcp)
                .filter_map(|(environment_id, name, _, endpoint)| {
                    let (address, path) = parse_address(&endpoint.address, MCP_PATH)?;
                    Some(Target {
                        environment_id,
                        name,
                        address,
                        path,
                        token: endpoint.token.map(|token| token.expose().to_owned()),
                        takes_session: true,
                    })
                })
                .collect();
        }
        host.running_servers()
            .await
            .into_iter()
            .filter(|(_, name, _)| name == server)
            .map(|(environment_id, name, endpoint)| Target {
                environment_id,
                name,
                address: endpoint.address,
                path: MCP_PATH.to_owned(),
                token: Some(endpoint.token),
                takes_session: false,
            })
            .collect()
    }

    /// Why the tool server `server` is not there in this environment.
    #[must_use]
    pub fn failure_of(&self, server: &str, environment: Id) -> Option<String> {
        let host = self.host.as_ref()?;
        host.mcp_failure(environment, server)
    }

    /// The target of an exchange must belong to its admitted environment.
    pub fn pick(targets: Vec<Target>, environment: Id) -> Option<Target> {
        targets
            .into_iter()
            .find(|target| target.environment_id == environment)
    }

    /// Checks what an exchange says of its session against the environment the tools act in, and states it
    /// as the executor reads it. What the ticket states is what counts; the rest is the other end's word,
    /// and is held to the environment that is attached and to this machine's policy.
    pub fn session_for(
        &self,
        header: Option<&str>,
        claims: Option<&TicketClaims>,
        environment_id: Id,
    ) -> Result<ExecScope, SessionRefusal> {
        use SessionRefusal::{Forbidden, Malformed};
        let header = header.ok_or_else(|| {
            Malformed("the exchange does not say which session it is in".to_owned())
        })?;
        let mut scope = ExecScope::decode(header).map_err(Malformed)?;
        if let Some(claims) = claims {
            let thread = claims
                .thread_id
                .ok_or_else(|| Forbidden("the ticket names no thread".to_owned()))?;
            scope.thread_id = thread.to_string();
            scope.tool_set = claims
                .tool_set
                .clone()
                .ok_or_else(|| Forbidden("the ticket names no tool set".to_owned()))?;
        }
        let host = self
            .host
            .as_ref()
            .ok_or_else(|| Forbidden("this machine plays no environment role".to_owned()))?;
        let environment = host
            .environment(environment_id)
            .map_err(|_| Forbidden("that environment is not attached here".to_owned()))?;
        let facts = environment.facts();
        // A session opened for an earlier making of the environment has no tools in this one: the system opens it
        // again for this one.
        if let Some(generation) = scope.generation
            && generation != facts.generation
        {
            return Err(Forbidden(format!(
                "the environment was made anew (generation {}) after this session was opened for generation {generation}; its tools are not this session's any more",
                facts.generation
            )));
        }
        let root = facts.workspace_root;
        if !lies_inside(&scope.directory, &root) {
            return Err(Forbidden(format!(
                "the directory {} is outside the workspace of the environment",
                scope.directory
            )));
        }
        if let Some(cwd) = &scope.cwd
            && !lies_inside(cwd, &root)
        {
            return Err(Forbidden(format!(
                "{cwd} is outside the workspace of the environment"
            )));
        }
        for mount in &mut scope.mounts {
            let attached = facts.mounts.iter().find(|attached| {
                attached.name == mount.name && attached.path.as_deref() == Some(mount.path.as_str())
            });
            let Some(attached) = attached else {
                return Err(Forbidden(format!(
                    "{} is no mount of the environment",
                    mount.name
                )));
            };
            // A mount attached for reading is read in whatever the session says.
            if attached.mode == Some(orch_link_contract::events::MountMode::ReadOnly) {
                mount.read_only = true;
            }
        }
        if let Some(policy) = self.services.policy.policy() {
            scope.sandbox_mode = policy.sandbox(scope.sandbox_mode);
        }
        Ok(scope)
    }
}

// ---------------------------------------------------------------------------------------------------------
// One HTTP exchange

fn bad_gateway(what: &str) -> Response<Body> {
    text(StatusCode::BAD_GATEWAY, what)
}

/// Forwards one request over `stream` to the target and gives its answer back as it comes, with the headers
/// `extra` added. The headers of the request that say who sent it are not passed; the target's token is.
pub async fn exchange<S>(
    stream: S,
    request: Request<Incoming>,
    target: &Target,
    extra: &[(&str, String)],
) -> Response<Body>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (parts, body) = request.into_parts();
    let body = Limited::new(body, REQUEST_MAX_BYTES);
    let mut outgoing = Request::builder()
        .method(parts.method)
        .uri(target.path.as_str())
        .header(hyper::header::HOST, target.address.to_string());
    for name in REQUEST_HEADERS {
        if let Some(value) = parts.headers.get(name) {
            outgoing = outgoing.header(name, value);
        }
    }
    if let Some(token) = &target.token {
        outgoing = outgoing.header(hyper::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    for (name, value) in extra {
        outgoing = outgoing.header(*name, value);
    }
    let Ok(outgoing) = outgoing.body(body) else {
        return bad_gateway("the request could not be passed on");
    };
    let handshake =
        hyper::client::conn::http1::handshake::<_, Limited<Incoming>>(TokioIo::new(stream)).await;
    let Ok((mut sender, connection)) = handshake else {
        return bad_gateway("the connection to the tool could not be made");
    };
    tokio::spawn(connection);
    let answered = match sender.send_request(outgoing).await {
        Ok(answered) => answered,
        Err(error) => return bad_gateway(&format!("the tool did not answer: {error}")),
    };
    let (answer, body) = answered.into_parts();
    let mut passed = Response::builder().status(answer.status);
    for name in RESPONSE_HEADERS {
        if let Some(value) = answer.headers.get(name) {
            passed = passed.header(name, value);
        }
    }
    passed
        .body(body.map_err(std::io::Error::other).boxed())
        .unwrap_or_else(|_| bad_gateway("the answer could not be passed on"))
}

/// A sender may repeat the signed environment, but cannot change it.
fn environment_header_matches(headers: &HeaderMap, environment: Id) -> bool {
    headers.get(ENVIRONMENT_HEADER).is_none_or(|value| {
        value.to_str().ok().and_then(|text| text.parse::<Id>().ok()) == Some(environment)
    })
}

impl ToolRelay {
    /// Forwards a request to a target on this machine: connects to it, with the executor started again once if it
    /// no longer answers, and says which session the exchange is in when the target asks.
    pub async fn forward_local(
        &self,
        request: Request<Incoming>,
        target: Target,
        session: Option<String>,
        generation: i64,
    ) -> Response<Body> {
        let connected = match TcpStream::connect(target.address).await {
            Ok(stream) => Ok((stream, target.clone())),
            Err(error) if target.takes_session => match self.restarted(&target).await {
                Some(again) => TcpStream::connect(again.address)
                    .await
                    .map(|stream| (stream, again))
                    .map_err(|e| e.to_string()),
                None => Err(error.to_string()),
            },
            Err(error) => Err(error.to_string()),
        };
        match connected {
            Ok((stream, target)) => {
                if !self.environment_matches(target.environment_id, generation) {
                    return text(StatusCode::FORBIDDEN, "the environment was made anew");
                }
                let extra: Vec<(&str, String)> = session
                    .into_iter()
                    .map(|session| (SESSION_HEADER, session))
                    .collect();
                exchange(stream, request, &target, &extra).await
            }
            Err(error) => bad_gateway(&format!("{} cannot be reached: {error}", target.name)),
        }
    }

    /// The executor of a target started again, where it was there once and no longer answers.
    pub async fn restarted(&self, target: &Target) -> Option<Target> {
        let host = self.host.as_ref()?;
        let endpoint = host
            .executor_ensure(target.environment_id, &target.name)
            .await
            .ok()?;
        let (address, path) = parse_address(&endpoint.address, &target.path)?;
        Some(Target {
            address,
            path,
            token: endpoint.token.map(|token| token.expose().to_owned()),
            ..target.clone()
        })
    }

    /// One exchange that came through a tunnel to `mcp/<server>`.
    async fn serve_exchange(
        &self,
        server: &str,
        claims: &TicketClaims,
        request: Request<Incoming>,
    ) -> Response<Body> {
        let Some((environment, generation)) = self.ticket_environment(claims) else {
            return text(
                StatusCode::FORBIDDEN,
                "the ticket's environment or generation is unavailable",
            );
        };
        if !environment_header_matches(request.headers(), environment) {
            return text(
                StatusCode::FORBIDDEN,
                "the request names another environment",
            );
        }
        let header = request
            .headers()
            .get(SESSION_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let targets = self.mcp_targets(server).await;
        let Some(target) = Self::pick(targets, environment) else {
            return match self.failure_of(server, environment) {
                Some(reason) => text(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &format!("the tool server {server} failed: {reason}"),
                ),
                None => text(
                    StatusCode::NOT_FOUND,
                    &format!("{server} is not running here"),
                ),
            };
        };
        let session = if target.takes_session {
            match self.session_for(header.as_deref(), Some(claims), target.environment_id) {
                Ok(scope) => Some(scope.encode()),
                Err(refusal) => return refusal.answer(),
            }
        } else {
            None
        };
        self.forward_local(request, target, session, generation)
            .await
    }
}

// ---------------------------------------------------------------------------------------------------------
// The WebSocket of an executor, carried in a tunnel

/// How a WebSocket of an executor is made: messages as large as the tunnel carries.
#[must_use]
pub fn websocket_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(EXEC_MESSAGE_MAX_BYTES))
        .max_frame_size(Some(EXEC_MESSAGE_MAX_BYTES))
}

/// Opens a WebSocket to an executor on this machine, presenting its token.
pub async fn connect_websocket(target: &Target) -> Result<WebSocketStream<TcpStream>, String> {
    let stream = TcpStream::connect(target.address)
        .await
        .map_err(|e| format!("{} cannot be reached: {e}", target.name))?;
    let mut request = format!("ws://{}{}", target.address, target.path)
        .into_client_request()
        .map_err(|e| e.to_string())?;
    if let Some(token) = &target.token
        && let Ok(value) = format!("Bearer {token}").parse()
    {
        request
            .headers_mut()
            .insert(hyper::header::AUTHORIZATION, value);
    }
    tokio_tungstenite::client_async_with_config(request, stream, Some(websocket_config()))
        .await
        .map(|(socket, _)| socket)
        .map_err(|e| format!("{} did not take the connection: {e}", target.name))
}

/// Writes a message into a tunnel as it is framed there.
async fn write_framed(tunnel: &mut Tunnel, opcode: ExecOpcode, payload: Bytes) -> bool {
    let mut framed = BytesMut::new();
    if (ExecMessage { opcode, payload })
        .write(&mut framed)
        .is_err()
    {
        return false;
    }
    tunnel.write_all(&framed).await.is_ok()
}

/// Carries the messages of a WebSocket into a tunnel and the messages of the tunnel into the WebSocket,
/// until either ends.
pub async fn pump<S>(mut socket: WebSocketStream<S>, mut tunnel: Tunnel)
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut inbox = BytesMut::new();
    let mut chunk = vec![0u8; TUNNEL_READ_BYTES];
    loop {
        tokio::select! {
            read = tunnel.read(&mut chunk) => match read {
                Ok(0) | Err(_) => {
                    let _ = socket.close(None).await;
                    return;
                }
                Ok(count) => {
                    inbox.extend_from_slice(&chunk[..count]);
                    loop {
                        match ExecMessage::take(&mut inbox, EXEC_MESSAGE_MAX_BYTES) {
                            Ok(Some(message)) => {
                                let sent = match message.opcode {
                                    ExecOpcode::Text => match String::from_utf8(message.payload.to_vec()) {
                                        Ok(text) => socket.send(Message::text(text)).await,
                                        Err(_) => {
                                            tunnel.fail("a text message is no text");
                                            return;
                                        }
                                    },
                                    ExecOpcode::Binary => socket.send(Message::binary(message.payload)).await,
                                };
                                if sent.is_err() {
                                    tunnel.close();
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                tunnel.fail(&error.to_string());
                                return;
                            }
                        }
                    }
                }
            },
            message = socket.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if !write_framed(&mut tunnel, ExecOpcode::Text, Bytes::copy_from_slice(text.as_bytes())).await {
                        return;
                    }
                }
                Some(Ok(Message::Binary(bytes))) => {
                    if !write_framed(&mut tunnel, ExecOpcode::Binary, bytes).await {
                        return;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    tunnel.close();
                    return;
                }
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Carries the messages of one WebSocket into another and back, until either ends.
pub async fn join_websockets<A, B>(mut first: WebSocketStream<A>, mut second: WebSocketStream<B>)
where
    A: AsyncRead + AsyncWrite + Unpin + Send,
    B: AsyncRead + AsyncWrite + Unpin + Send,
{
    loop {
        tokio::select! {
            message = first.next() => match message {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    if second.send(message).await.is_err() {
                        let _ = first.close(None).await;
                        return;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    let _ = second.close(None).await;
                    return;
                }
                Some(Ok(_)) => {}
            },
            message = second.next() => match message {
                Some(Ok(message @ (Message::Text(_) | Message::Binary(_)))) => {
                    if first.send(message).await.is_err() {
                        let _ = second.close(None).await;
                        return;
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    let _ = first.close(None).await;
                    return;
                }
                Some(Ok(_)) => {}
            },
        }
    }
}

// ---------------------------------------------------------------------------------------------------------
// Tunnels that end here

/// Serves a tunnel to an executor or a tool server of this machine.
pub async fn serve(relay: &Arc<ToolRelay>, incoming: IncomingTunnel) {
    match incoming.service.parse::<Service>() {
        Ok(Service::Mcp { server }) => serve_mcp(relay, incoming, server).await,
        Ok(Service::Exec { executor }) => serve_exec(relay, incoming, executor).await,
        _ => incoming.refuse(
            OpenErrorCode::NoSuchService,
            "this machine serves executors under exec/<kind> and tool servers under mcp/<name>",
        ),
    }
}

async fn serve_mcp(relay: &Arc<ToolRelay>, incoming: IncomingTunnel, server: String) {
    let Some((environment, _generation)) = relay.ticket_environment(&incoming.claims) else {
        incoming.refuse(
            OpenErrorCode::BadTicket,
            "the ticket's environment or generation is unavailable",
        );
        return;
    };
    if ToolRelay::pick(relay.mcp_targets(&server).await, environment).is_none() {
        match relay.failure_of(&server, environment) {
            Some(reason) => incoming.refuse(
                OpenErrorCode::Refused,
                &format!("the tool server {server} failed: {reason}"),
            ),
            None => incoming.refuse(
                OpenErrorCode::NoSuchService,
                &format!("{server} is not running here"),
            ),
        }
        return;
    }
    let claims = incoming.claims.clone();
    let tunnel = match incoming.accept() {
        Ok(tunnel) => tunnel,
        Err(error) => {
            tracing::debug!(%error, "a tunnel to a tool could not be taken up");
            return;
        }
    };
    let relay = Arc::clone(relay);
    let service = service_fn(move |request: Request<Incoming>| {
        let relay = Arc::clone(&relay);
        let server = server.clone();
        let claims = claims.clone();
        async move { Ok::<_, Infallible>(relay.serve_exchange(&server, &claims, request).await) }
    });
    let served = hyper::server::conn::http1::Builder::new()
        .serve_connection(TokioIo::new(tunnel), service)
        .await;
    if let Err(error) = served {
        tracing::debug!(%error, "an exchange with a tool ended badly");
    }
}

async fn serve_exec(relay: &Arc<ToolRelay>, incoming: IncomingTunnel, executor: String) {
    let Some((environment, generation)) = relay.ticket_environment(&incoming.claims) else {
        incoming.refuse(
            OpenErrorCode::BadTicket,
            "the ticket's environment or generation is unavailable",
        );
        return;
    };
    // An executor that offers its tools as an MCP server is reached by HTTP exchanges, whichever of the
    // two services the system named in the ticket of the session.
    if relay.offers_mcp(&executor, environment).await {
        serve_mcp(relay, incoming, MCP_SERVER_NAME.to_owned()).await;
        return;
    }
    let targets = relay.exec_targets(&executor).await;
    let Some(target) = ToolRelay::pick(targets, environment) else {
        incoming.refuse(
            OpenErrorCode::NoSuchService,
            &format!("{executor} is not running here"),
        );
        return;
    };
    let socket = match connect_websocket(&target).await {
        Ok(socket) => socket,
        Err(first) => {
            let again = relay.restarted(&target).await;
            match again {
                Some(again) => match connect_websocket(&again).await {
                    Ok(socket) => socket,
                    Err(error) => {
                        incoming.refuse(OpenErrorCode::Refused, &error);
                        return;
                    }
                },
                None => {
                    incoming.refuse(OpenErrorCode::Refused, &first);
                    return;
                }
            }
        }
    };
    if !relay.environment_matches(environment, generation) {
        incoming.refuse(OpenErrorCode::BadTicket, "the environment was made anew");
        return;
    }
    match incoming.accept() {
        Ok(tunnel) => pump(socket, tunnel).await,
        Err(error) => tracing::debug!(%error, "a tunnel to an executor could not be taken up"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_an_executor_was_started_at_is_taken_apart() {
        let (address, path) = parse_address("http://127.0.0.1:41234/mcp", "/other").unwrap();
        assert_eq!((address.port(), path.as_str()), (41234, "/mcp"));
        let (address, path) = parse_address("ws://127.0.0.1:7000", EXEC_SERVER_PATH).unwrap();
        assert_eq!((address.port(), path.as_str()), (7000, "/"));
        assert!(parse_address("10.0.0.5:7000", "/").is_none());
        assert!(parse_address("127.0.0.1", "/").is_none());
    }

    #[test]
    fn a_directory_is_inside_a_workspace_only_when_it_climbs_nowhere() {
        assert!(lies_inside("/work/web/api", "/work"));
        assert!(lies_inside("/work", "/work"));
        assert!(!lies_inside("/workshop", "/work"));
        assert!(!lies_inside("/work/../etc", "/work"));
        assert!(!lies_inside("/etc", "/work"));
    }

    #[test]
    fn a_session_survives_the_header_it_travels_in() {
        let scope = ExecScope {
            thread_id: "t-1".to_owned(),
            tool_set: "plain".to_owned(),
            directory: "/w/ünï".to_owned(),
            cwd: None,
            sandbox_mode: SandboxMode::ReadOnly,
            mounts: vec![ExecMount {
                name: "src".to_owned(),
                path: "/data".to_owned(),
                read_only: true,
            }],
            generation: Some(3),
        };
        assert_eq!(ExecScope::decode(&scope.encode()), Ok(scope));
        assert!(ExecScope::decode("not base64 !!").is_err());
    }
}
