//! What a harness on this machine reaches its tools at: its connection to its executor, `/exec/<session key>`, and its tool servers, `/mcp/<session key>/<name>`, each carried to where the tool runs.
//!
//! The listener has checked the bearer token of the harness process and found the session by its key
//! before a request gets here. The tool is then either on this machine, in an environment that is
//! attached here, and the request goes to it directly, or on another machine, and it goes through a tunnel
//! opened with the ticket the system signed for the session:
//!
//! * `/exec/<key>` is a WebSocket the harness opens to its executor (Codex's exec server). The messages
//!   go to the executor's own WebSocket, or into an `exec` tunnel, framed one after the other.
//! * `/mcp/<key>/exec` is Orch's own executor, offered as a tool server, and `/mcp/<key>/<name>` is a tool
//!   server of the environment. Each request is one HTTP exchange: passed to the tool's address, or put into
//!   an `mcp` tunnel of its own. The token the harness presented to this listener is its own and goes
//!   nowhere; the session it is in, and the environment its tools act in, are said in headers, which the
//!   other end checks against what the ticket states.
//!
//! The harness is never told which of the two it is: both are the same address on its own machine.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use async_trait::async_trait;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_agent_contract::loopback::Route;
use orch_link::tunnels::channel::OpenError;
use orch_link_contract::tunnels::OpenErrorCode;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::derive_accept_key;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::loopback::{Body, Endpoint, SessionBinding, text};
use crate::tool_tunnels::{
    ENVIRONMENT_HEADER, ExecMount, ExecScope, SESSION_HEADER, Target, ToolRelay, connect_websocket,
    exchange, join_websockets, pump, websocket_config,
};

/// The name of Orch's own executor as a tool server.
const EXEC_SERVER: &str = orch_envhost::executors::MCP_SERVER_NAME;

/// The name of Orch's own tools as a tool server: presented by the connector, and no part of this module.
const ORCH_SERVER: &str = "orch";

/// The path an exchange travels under inside a tunnel: the other end forwards to its own.
const TUNNEL_PATH: &str = orch_envhost::mcp_host::MCP_PATH;

/// What the endpoint for `/exec/<key>` is made of.
pub struct ExecEndpoint {
    relay: Arc<ToolRelay>,
}

/// What the endpoint for `/mcp/<key>/<name>` is made of.
pub struct McpEndpoint {
    relay: Arc<ToolRelay>,
}

impl ExecEndpoint {
    /// The endpoint, over the relay of this connector.
    #[must_use]
    pub fn new(relay: Arc<ToolRelay>) -> Self {
        Self { relay }
    }
}

impl McpEndpoint {
    /// The endpoint, over the relay of this connector.
    #[must_use]
    pub fn new(relay: Arc<ToolRelay>) -> Self {
        Self { relay }
    }
}

fn open_failure(error: &OpenError) -> Response<Body> {
    match error {
        OpenError::Refused { code, detail } => {
            let status = match code {
                OpenErrorCode::PeerOffline => StatusCode::SERVICE_UNAVAILABLE,
                OpenErrorCode::NoSuchService => StatusCode::NOT_FOUND,
                OpenErrorCode::BadTicket | OpenErrorCode::Expired => StatusCode::FORBIDDEN,
                _ => StatusCode::BAD_GATEWAY,
            };
            text(status, detail)
        }
        other => text(StatusCode::BAD_GATEWAY, &other.to_string()),
    }
}

/// The session as the executor reads it, from what the session was opened with.
fn scope_of(session: &SessionBinding, reach: &crate::loopback::Reach) -> Option<ExecScope> {
    Some(ExecScope {
        thread_id: session.thread_id.to_string(),
        tool_set: reach.tool_set.clone()?,
        directory: reach.directory.clone(),
        cwd: Some(reach.cwd.clone()),
        sandbox_mode: reach.sandbox_mode,
        mounts: reach
            .mounts
            .iter()
            .map(|mount| ExecMount {
                name: mount.name.clone(),
                path: mount.path.clone(),
                read_only: mount.read_only,
            })
            .collect(),
        generation: Some(reach.generation),
    })
}

#[async_trait]
impl Endpoint for McpEndpoint {
    async fn serve(
        &self,
        request: Request<Incoming>,
        session: Arc<SessionBinding>,
        route: Route,
    ) -> Response<Body> {
        let Route::Mcp { server, .. } = route else {
            return text(StatusCode::NOT_FOUND, "no such address");
        };
        if server == ORCH_SERVER {
            return text(
                StatusCode::NOT_IMPLEMENTED,
                "nothing serves this address on this connector",
            );
        }
        let Some(reach) = self.relay.reach_of(&session.session_key) else {
            return text(
                StatusCode::NOT_FOUND,
                "this session has no tools of an environment",
            );
        };
        let is_exec = server == EXEC_SERVER;
        if !is_exec && !session.tickets.mcp.contains_key(&server) {
            return text(
                StatusCode::NOT_FOUND,
                &format!("this session was given no way to reach {server}"),
            );
        }
        let scope = if is_exec {
            match scope_of(&session, &reach) {
                Some(scope) => Some(scope),
                None => {
                    return text(
                        StatusCode::NOT_FOUND,
                        "this session has no executor that offers tools",
                    );
                }
            }
        } else {
            None
        };

        // The tool is here: the environment it acts in is attached on this machine.
        if self.relay.has_environment(reach.environment_id) {
            if !self
                .relay
                .environment_matches(reach.environment_id, reach.generation)
            {
                return text(
                    StatusCode::NOT_FOUND,
                    "this session's environment was made anew",
                );
            }
            let targets = self.relay.mcp_targets(&server).await;
            let target = ToolRelay::pick(targets, reach.environment_id);
            let Some(target) = target else {
                return match self.relay.failure_of(&server, reach.environment_id) {
                    Some(reason) => text(
                        StatusCode::SERVICE_UNAVAILABLE,
                        &format!("the tool server {server} failed: {reason}"),
                    ),
                    None => text(
                        StatusCode::NOT_FOUND,
                        &format!("{server} is not running in this environment"),
                    ),
                };
            };
            let header = match scope {
                Some(scope) if target.takes_session => {
                    match self
                        .relay
                        .session_for(Some(&scope.encode()), None, reach.environment_id)
                    {
                        Ok(checked) => Some(checked.encode()),
                        Err(refusal) => return refusal.answer(),
                    }
                }
                _ => None,
            };
            return self
                .relay
                .forward_local(request, target, header, reach.generation)
                .await;
        }

        // The tool is on another machine: through a tunnel of the ticket the system signed.
        let ticket = if is_exec {
            session.tickets.exec.clone()
        } else {
            session.tickets.mcp.get(&server).cloned()
        };
        let Some(ticket) = ticket else {
            return text(
                StatusCode::NOT_FOUND,
                &format!("this session was given no way to reach {server}"),
            );
        };
        let tunnel = match self.relay.link().open_tunnel(&ticket).await {
            Ok(tunnel) => tunnel,
            Err(error) => return open_failure(&error),
        };
        let mut extra = vec![(ENVIRONMENT_HEADER, reach.environment_id.to_string())];
        if let Some(scope) = scope {
            extra.push((SESSION_HEADER, scope.encode()));
        }
        let beyond = Target {
            environment_id: reach.environment_id,
            name: server,
            address: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            path: TUNNEL_PATH.to_owned(),
            token: None,
            takes_session: false,
        };
        exchange(tunnel, request, &beyond, &extra).await
    }
}

/// Whether a request asks for its connection to be turned into a WebSocket.
fn asks_for_a_websocket(request: &Request<Incoming>) -> Option<String> {
    let headers = request.headers();
    let says = |name: &str, word: &str| {
        headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| {
                value
                    .split(',')
                    .any(|part| part.trim().eq_ignore_ascii_case(word))
            })
    };
    if request.method() != hyper::Method::GET
        || !says("upgrade", "websocket")
        || !says("connection", "upgrade")
    {
        return None;
    }
    headers
        .get("sec-websocket-key")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

/// What the harness's messages are carried to.
enum Upstream {
    /// The executor's own WebSocket, on this machine; boxed, for it is far larger than a tunnel.
    Here(Box<WebSocketStream<tokio::net::TcpStream>>),
    /// An `exec` tunnel to the executor on another machine.
    Through(orch_link::tunnels::channel::Tunnel),
}

#[async_trait]
impl Endpoint for ExecEndpoint {
    async fn serve(
        &self,
        mut request: Request<Incoming>,
        session: Arc<SessionBinding>,
        route: Route,
    ) -> Response<Body> {
        if !matches!(route, Route::Exec { .. }) {
            return text(StatusCode::NOT_FOUND, "no such address");
        }
        let Some(key) = asks_for_a_websocket(&request) else {
            return text(
                StatusCode::UPGRADE_REQUIRED,
                "this address takes a WebSocket",
            );
        };
        let Some(reach) = self.relay.reach_of(&session.session_key) else {
            return text(
                StatusCode::NOT_FOUND,
                "this session has no tools of an environment",
            );
        };
        let Some(executor) = reach.executor.clone() else {
            return text(
                StatusCode::NOT_FOUND,
                "this session carries out its own tools",
            );
        };
        // The executor is reached before the harness is told it is: a harness whose executor is not there
        // learns it from the answer to its request, not from a connection that closes.
        let upstream = if self.relay.has_environment(reach.environment_id) {
            if !self
                .relay
                .environment_matches(reach.environment_id, reach.generation)
            {
                return text(
                    StatusCode::NOT_FOUND,
                    "this session's environment was made anew",
                );
            }
            let targets = self.relay.exec_targets(&executor).await;
            let target = ToolRelay::pick(targets, reach.environment_id);
            let Some(target) = target else {
                return text(
                    StatusCode::NOT_FOUND,
                    &format!("{executor} is not running in this environment"),
                );
            };
            match connect_websocket(&target).await {
                Ok(socket) => Upstream::Here(Box::new(socket)),
                Err(first) => {
                    let Some(again) = self.relay.restarted(&target).await else {
                        return text(StatusCode::BAD_GATEWAY, &first);
                    };
                    match connect_websocket(&again).await {
                        Ok(socket) => Upstream::Here(Box::new(socket)),
                        Err(error) => return text(StatusCode::BAD_GATEWAY, &error),
                    }
                }
            }
        } else {
            let Some(ticket) = session.tickets.exec.clone() else {
                return text(
                    StatusCode::NOT_FOUND,
                    "this session was given no way to reach its executor",
                );
            };
            match self.relay.link().open_tunnel(&ticket).await {
                Ok(tunnel) => Upstream::Through(tunnel),
                Err(error) => return open_failure(&error),
            }
        };
        if matches!(&upstream, Upstream::Here(_))
            && !self
                .relay
                .environment_matches(reach.environment_id, reach.generation)
        {
            return text(
                StatusCode::NOT_FOUND,
                "this session's environment was made anew",
            );
        }
        let upgrade = hyper::upgrade::on(&mut request);
        tokio::spawn(async move {
            let Ok(upgraded) = upgrade.await else {
                return;
            };
            let harness = WebSocketStream::from_raw_socket(
                TokioIo::new(upgraded),
                Role::Server,
                Some(websocket_config()),
            )
            .await;
            match upstream {
                Upstream::Here(socket) => join_websockets(harness, *socket).await,
                Upstream::Through(tunnel) => pump(harness, tunnel).await,
            }
        });
        let mut response = text(StatusCode::SWITCHING_PROTOCOLS, "");
        let headers = response.headers_mut();
        headers.remove(hyper::header::CONTENT_TYPE);
        headers.insert(
            hyper::header::UPGRADE,
            hyper::header::HeaderValue::from_static("websocket"),
        );
        headers.insert(
            hyper::header::CONNECTION,
            hyper::header::HeaderValue::from_static("Upgrade"),
        );
        if let Ok(accept) = derive_accept_key(key.as_bytes()).parse() {
            headers.insert("sec-websocket-accept", accept);
        }
        response
    }
}
