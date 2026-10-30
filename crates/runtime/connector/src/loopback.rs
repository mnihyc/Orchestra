//! Session-keyed model, executor, MCP and Orch-tool endpoints; Orch's own tools, declared with their schemas, titles and hints, beside every executor unless the environment says otherwise.
//!
//! Everything a harness reaches beyond itself goes through one HTTP listener of the connector, bound to
//! `127.0.0.1` on a port the operating system chooses and to nothing else. This module is the listener
//! and the routing. What lies behind the routes arrives with the roles that need them: the model
//! forwarder, the executor's connection, the tool servers and Orch's own tools are **endpoints**, each
//! registered for a kind of route with [`Loopback::register`], and each handed the request together with
//! the session it belongs to.
//!
//! A request is let through only if it is the listener's own and belongs to a session:
//!
//! * its `Host` is `127.0.0.1:<port>` and nothing else, so that a web page cannot reach the listener by
//!   a name it controls;
//! * it carries `Authorization: Bearer <token>`, the token of one harness process (256 random bits,
//!   given to that process when it is started), compared without leaking where it differs;
//! * its path is one of the forms of `orch_agent_contract::loopback`, and its session key (128 random
//!   bits) names a session that belongs to the harness process whose token was presented. A key of
//!   another process's session is answered as if it did not exist.
//!
//! A route no endpoint is registered for is answered `501`: the role that serves it is not built here.

use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_agent_contract::loopback::{Route, host_is_allowed, parse_path};
use orch_plugin_contract::Secret;
use orch_types::Id;
use rand::RngExt as _;
use subtle::ConstantTimeEq;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// How many random bytes make the token of a harness process: 256 bits.
const TOKEN_BYTES: usize = 32;
/// How many random bytes make the key of a session: 128 bits.
const SESSION_KEY_BYTES: usize = 16;

/// The body of a response an endpoint makes.
pub type Body = BoxBody<Bytes, std::io::Error>;

/// A response with a short text and a status.
#[must_use]
pub fn text(status: StatusCode, message: &str) -> Response<Body> {
    let mut response = Response::new(
        Full::new(Bytes::from(message.to_owned()))
            .map_err(|never: Infallible| match never {})
            .boxed(),
    );
    *response.status_mut() = status;
    response.headers_mut().insert(
        hyper::header::CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// What a session was given to reach other machines: the tickets that open its tunnels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tickets {
    /// For the model calls of a forwarded session.
    pub llm: Option<String>,
    /// For the connection to the executor.
    pub exec: Option<String>,
    /// For each tool server of the environment, by name.
    pub mcp: BTreeMap<String, String>,
}

/// A session as the listener knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBinding {
    /// The key its addresses are under.
    pub session_key: String,
    /// The harness process it belongs to.
    pub grant: u64,
    /// The thread.
    pub thread_id: Id,
    /// The stretch of the thread.
    pub segment_id: Id,
    /// The provider of the harness process.
    pub provider_id: Id,
    /// The agent profile of the harness process.
    pub agent_id: Id,
    /// What it may open tunnels with.
    pub tickets: Tickets,
}

/// The kinds of route an endpoint can be registered for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EndpointKind {
    /// `/llm/<key>/…`.
    Llm,
    /// `/exec/<key>`.
    Exec,
    /// `/mcp/<key>/<server>` for this server.
    Mcp(String),
    /// `/mcp/<key>/<server>` for any server no endpoint of its own is registered for.
    AnyMcp,
}

/// Something that serves a kind of route.
#[async_trait]
pub trait Endpoint: Send + Sync {
    /// Serves a request that is the listener's own and belongs to `session`.
    async fn serve(
        &self,
        request: Request<Incoming>,
        session: Arc<SessionBinding>,
        route: Route,
    ) -> Response<Body>;
}

/// The token of one harness process. Dropping it does not revoke it: [`Loopback::revoke`] does.
#[derive(Debug, Clone)]
pub struct Grant {
    /// Names the process to the listener.
    pub id: u64,
    /// What the process presents; it is passed to the harness in its environment.
    pub token: Secret,
}

#[derive(Default)]
struct State {
    next_grant: u64,
    grants: HashMap<u64, Secret>,
    sessions: HashMap<String, Arc<SessionBinding>>,
    endpoints: HashMap<EndpointKind, Arc<dyn Endpoint>>,
    reaches: HashMap<String, Reach>,
    environment_ports: HashMap<Id, String>,
}

struct Shared {
    port: u16,
    state: Mutex<State>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The connector's listener for the harnesses it hosts.
pub struct Loopback {
    shared: Arc<Shared>,
    stop: CancellationToken,
}

impl std::fmt::Debug for Loopback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loopback")
            .field("port", &self.shared.port)
            .finish_non_exhaustive()
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rng().fill(&mut buffer[..]);
    hex::encode(buffer)
}

impl Loopback {
    /// Binds the listener on `127.0.0.1` at a port the system chooses, and serves until [`Loopback::stop`].
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).await?;
        let port = listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            port,
            state: Mutex::new(State::default()),
        });
        let stop = CancellationToken::new();
        tokio::spawn(accept(listener, Arc::clone(&shared), stop.clone()));
        Ok(Self { shared, stop })
    }

    /// The port the listener is on.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// Publishes only a managed unit's local URL for its native harness process environment.
    /// Connected environment tokens are never placed on a shared harness process.
    pub fn environment_ports(&self, environment: Id, url: Option<String>) {
        let mut state = self.shared.state();
        if let Some(url) = url {
            state.environment_ports.insert(environment, url);
        } else {
            state.environment_ports.remove(&environment);
        }
    }

    /// A managed unit has one environment; an ambiguous mapping is not another environment's URL.
    #[must_use]
    pub fn environment_ports_url(&self) -> Option<String> {
        let state = self.shared.state();
        (state.environment_ports.len() == 1)
            .then(|| state.environment_ports.values().next().cloned())
            .flatten()
    }

    /// Registers what serves a kind of route, in place of what served it.
    pub fn register(&self, kind: EndpointKind, endpoint: Arc<dyn Endpoint>) {
        self.shared.state().endpoints.insert(kind, endpoint);
    }

    /// Makes the token of a harness process.
    #[must_use]
    pub fn grant(&self) -> Grant {
        let token = Secret::new(random_hex(TOKEN_BYTES));
        let mut state = self.shared.state();
        state.next_grant += 1;
        let id = state.next_grant;
        state.grants.insert(id, token.clone());
        Grant { id, token }
    }

    /// Takes a token back, and the sessions that belong to it.
    pub fn revoke(&self, grant: u64) {
        let mut state = self.shared.state();
        state.grants.remove(&grant);
        state.sessions.retain(|_, session| session.grant != grant);
        let State {
            sessions, reaches, ..
        } = &mut *state;
        reaches.retain(|key, _| sessions.contains_key(key));
    }

    /// Makes the key of a new session of the process `grant` and records what it was given.
    #[must_use]
    pub fn bind(
        &self,
        grant: u64,
        thread_id: Id,
        segment_id: Id,
        provider_id: Id,
        agent_id: Id,
        tickets: Tickets,
    ) -> Arc<SessionBinding> {
        let session_key = random_hex(SESSION_KEY_BYTES);
        let binding = Arc::new(SessionBinding {
            session_key: session_key.clone(),
            grant,
            thread_id,
            segment_id,
            provider_id,
            agent_id,
            tickets,
        });
        self.shared
            .state()
            .sessions
            .insert(session_key, Arc::clone(&binding));
        binding
    }

    /// Forgets a session: its addresses are answered as if they never were.
    /// Changes only MCP tickets at the same session key; callers in flight keep their previous binding.
    pub fn replace_mcp(
        &self,
        session_key: &str,
        tickets: BTreeMap<String, String>,
    ) -> Option<BTreeMap<String, String>> {
        let mut state = self.shared.state();
        let current = state.sessions.get(session_key)?.as_ref().clone();
        let mut changed = current;
        let previous = std::mem::replace(&mut changed.tickets.mcp, tickets);
        state
            .sessions
            .insert(session_key.to_owned(), Arc::new(changed));
        Some(previous)
    }

    /// Forgets a session: its addresses are answered as if they never were.
    pub fn unbind(&self, session_key: &str) {
        let mut state = self.shared.state();
        state.sessions.remove(session_key);
        state.reaches.remove(session_key);
    }

    /// The session of a key.
    #[must_use]
    pub fn session(&self, session_key: &str) -> Option<Arc<SessionBinding>> {
        self.shared.state().sessions.get(session_key).cloned()
    }

    /// Stops listening.
    pub fn stop(&self) {
        self.stop.cancel();
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.cancel();
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
                .serve_connection(TokioIo::new(stream), service)
                .with_upgrades();
            tokio::pin!(connection);
            tokio::select! {
                () = stop.cancelled() => {}
                _ = &mut connection => {}
            }
        });
    }
}

fn bearer(request: &Request<Incoming>) -> Option<&str> {
    request
        .headers()
        .get(hyper::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn handle(shared: &Shared, request: Request<Incoming>) -> Response<Body> {
    let host_ok = request
        .headers()
        .get(hyper::header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| host_is_allowed(host, shared.port));
    if !host_ok {
        return text(
            StatusCode::MISDIRECTED_REQUEST,
            "this listener answers to 127.0.0.1 only",
        );
    }
    let presented = bearer(&request).unwrap_or_default().as_bytes().to_vec();
    let grant = {
        let state = shared.state();
        let mut found = None;
        for (id, token) in &state.grants {
            if bool::from(token.expose().as_bytes().ct_eq(&presented)) {
                found = Some(*id);
            }
        }
        found
    };
    let Some(grant) = grant else {
        let mut response = text(StatusCode::UNAUTHORIZED, "a bearer token is required");
        response.headers_mut().insert(
            hyper::header::WWW_AUTHENTICATE,
            hyper::header::HeaderValue::from_static("Bearer"),
        );
        return response;
    };
    let Some(route) = parse_path(request.uri().path()) else {
        return text(StatusCode::NOT_FOUND, "no such address");
    };
    let key = match &route {
        Route::Llm { session_key, .. }
        | Route::Exec { session_key }
        | Route::Mcp { session_key, .. } => session_key.clone(),
    };
    let (session, endpoint) = {
        let state = shared.state();
        let session = state
            .sessions
            .get(&key)
            .filter(|session| session.grant == grant)
            .cloned();
        let kind = match &route {
            Route::Llm { .. } => EndpointKind::Llm,
            Route::Exec { .. } => EndpointKind::Exec,
            Route::Mcp { server, .. } => EndpointKind::Mcp(server.clone()),
        };
        let endpoint = state.endpoints.get(&kind).cloned().or_else(|| {
            matches!(route, Route::Mcp { .. })
                .then(|| state.endpoints.get(&EndpointKind::AnyMcp).cloned())
                .flatten()
        });
        (session, endpoint)
    };
    let Some(session) = session else {
        return text(StatusCode::NOT_FOUND, "no such address");
    };
    match endpoint {
        Some(endpoint) => endpoint.serve(request, session, route).await,
        None => text(
            StatusCode::NOT_IMPLEMENTED,
            "nothing serves this address on this connector",
        ),
    }
}

// ---------------------------------------------------------------------------------------------------------
// Where a session's tools are

pub use reach::{Reach, ReachMount};

/// What the endpoints for a session's tools need to know of it beyond its key and its tickets: which
/// environment its tools act in, which executor and tool set carry them out, and what they are held to.
mod reach {
    use super::{Id, Loopback};
    use orch_link_contract::commands::{OrchTool, SandboxMode, SessionOpen};

    /// A mount the tools of a session may reach, as the system named it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ReachMount {
        /// Its name.
        pub name: String,
        /// Where it is in the environment.
        pub path: String,
        /// Whether nothing may write there.
        pub read_only: bool,
    }

    /// Where the tools of a session are carried out and what they are held to.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Reach {
        /// The environment the tools act in.
        pub environment_id: Id,
        /// The executor that carries them out; `None` for a harness that carries out its own.
        pub executor: Option<String>,
        /// The tool set of Orch's own executor the session has.
        pub tool_set: Option<String>,
        /// The thread's directory in its environment.
        pub directory: String,
        /// Where the agent starts inside it.
        pub cwd: String,
        /// The mounts that are attached.
        pub mounts: Vec<ReachMount>,
        /// The sandbox mode of the turn that runs, or of the session until a turn is started.
        pub sandbox_mode: SandboxMode,
        /// The tools of the `orch` server the session was opened with.
        pub orch_tools: Vec<OrchTool>,
        /// The descriptions a place set for them, by tool name, in place of the shipped ones.
        pub orch_tool_descriptions: std::collections::BTreeMap<String, String>,
        /// The current policy-narrowed schemas, carried with the session setup.
        pub orch_tool_schemas: std::collections::BTreeMap<String, orch_types::Json>,
        /// How many times the environment was made anew when the session was opened: once it is made again, the
        /// session's tools are refused there.
        pub generation: i64,
    }

    impl Reach {
        /// The reach a `session.open` gives a session.
        #[must_use]
        pub fn of(open: &SessionOpen) -> Self {
            Self {
                environment_id: open.exec.environment_id,
                executor: open.exec.executor.clone(),
                tool_set: open.exec.tool_set.clone(),
                directory: open.directory.clone(),
                cwd: open.cwd.clone(),
                mounts: open
                    .mounts
                    .iter()
                    .map(|mount| ReachMount {
                        name: mount.name.clone(),
                        path: mount.path.clone(),
                        read_only: mount.read_only,
                    })
                    .collect(),
                sandbox_mode: open.sandbox_mode,
                orch_tools: open.orch_tools.clone(),
                orch_tool_descriptions: open.orch_tool_descriptions.clone(),
                orch_tool_schemas: open.orch_tool_schemas.clone(),
                generation: open.exec.generation,
            }
        }
    }

    impl Loopback {
        /// Records where the tools of a session are.
        pub fn set_reach(&self, session_key: &str, reach: Reach) {
            self.shared
                .state()
                .reaches
                .insert(session_key.to_owned(), reach);
        }

        /// Where the tools of a session are, while the session is bound.
        #[must_use]
        pub fn reach(&self, session_key: &str) -> Option<Reach> {
            self.shared.state().reaches.get(session_key).cloned()
        }

        /// Says that a turn of a thread runs in `sandbox_mode`: the tools of its session are held to that.
        pub fn note_turn(&self, thread: Id, sandbox_mode: SandboxMode) {
            let mut state = self.shared.state();
            let keys: Vec<String> = state
                .sessions
                .values()
                .filter(|session| session.thread_id == thread)
                .map(|session| session.session_key.clone())
                .collect();
            for key in keys {
                if let Some(reach) = state.reaches.get_mut(&key) {
                    reach.sandbox_mode = sandbox_mode;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    struct Echo;

    #[async_trait]
    impl Endpoint for Echo {
        async fn serve(
            &self,
            _request: Request<Incoming>,
            session: Arc<SessionBinding>,
            route: Route,
        ) -> Response<Body> {
            text(
                StatusCode::OK,
                &format!("{} {:?}", session.thread_id, route),
            )
        }
    }

    async fn get(port: u16, host: &str, path: &str, token: Option<&str>) -> (StatusCode, String) {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let mut request = Request::builder().uri(path).header("host", host);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let response = sender
            .send_request(request.body(Full::new(Bytes::new())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    struct Rig {
        loopback: Loopback,
        grant: Grant,
        binding: Arc<SessionBinding>,
    }

    async fn rig() -> Rig {
        let loopback = Loopback::start().await.unwrap();
        let grant = loopback.grant();
        let binding = loopback.bind(
            grant.id,
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            Tickets::default(),
        );
        Rig {
            loopback,
            grant,
            binding,
        }
    }

    fn host(rig: &Rig) -> String {
        format!("127.0.0.1:{}", rig.loopback.port())
    }

    #[tokio::test]
    async fn the_listener_is_on_the_machine_itself_at_a_port_the_system_chose() {
        let rig = rig().await;
        assert_ne!(rig.loopback.port(), 0);
        let local = tokio::net::TcpStream::connect(("127.0.0.1", rig.loopback.port())).await;
        assert!(local.is_ok());
    }

    #[tokio::test]
    async fn a_request_needs_the_token_of_a_harness_process() {
        let rig = rig().await;
        rig.loopback.register(EndpointKind::AnyMcp, Arc::new(Echo));
        let path = format!("/mcp/{}/orch", rig.binding.session_key);
        let (status, _) = get(rig.loopback.port(), &host(&rig), &path, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, _) = get(rig.loopback.port(), &host(&rig), &path, Some("wrong")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = get(
            rig.loopback.port(),
            &host(&rig),
            &path,
            Some(rig.grant.token.expose()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            body.starts_with(&rig.binding.thread_id.to_string()),
            "{body}"
        );
    }

    #[tokio::test]
    async fn a_name_other_than_127_0_0_1_is_not_answered() {
        let rig = rig().await;
        let path = format!("/exec/{}", rig.binding.session_key);
        for wrong in ["localhost", "evil.example.com", "127.0.0.1"] {
            let (status, _) = get(
                rig.loopback.port(),
                wrong,
                &path,
                Some(rig.grant.token.expose()),
            )
            .await;
            assert_eq!(status, StatusCode::MISDIRECTED_REQUEST, "{wrong}");
        }
    }

    #[tokio::test]
    async fn a_session_is_reached_only_with_the_token_of_its_own_harness_process() {
        let rig = rig().await;
        rig.loopback.register(EndpointKind::Exec, Arc::new(Echo));
        let other = rig.loopback.grant();
        let path = format!("/exec/{}", rig.binding.session_key);
        let (status, _) = get(
            rig.loopback.port(),
            &host(&rig),
            &path,
            Some(other.token.expose()),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a key of another process's session is answered as if it did not exist"
        );
        let unknown = format!("/exec/{}", "0".repeat(32));
        let (status, _) = get(
            rig.loopback.port(),
            &host(&rig),
            &unknown,
            Some(rig.grant.token.expose()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = get(
            rig.loopback.port(),
            &host(&rig),
            "/files/x",
            Some(rig.grant.token.expose()),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_route_nothing_serves_is_answered_as_not_built_and_a_server_by_name_wins_over_any() {
        let rig = rig().await;
        let token = rig.grant.token.expose().to_owned();
        let llm = format!("/llm/{}/v1/responses", rig.binding.session_key);
        let (status, _) = get(rig.loopback.port(), &host(&rig), &llm, Some(&token)).await;
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
        struct Named;
        #[async_trait]
        impl Endpoint for Named {
            async fn serve(
                &self,
                _: Request<Incoming>,
                _: Arc<SessionBinding>,
                _: Route,
            ) -> Response<Body> {
                text(StatusCode::OK, "named")
            }
        }
        rig.loopback.register(EndpointKind::AnyMcp, Arc::new(Echo));
        rig.loopback
            .register(EndpointKind::Mcp("orch".into()), Arc::new(Named));
        let orch = format!("/mcp/{}/orch", rig.binding.session_key);
        let other = format!("/mcp/{}/tmux", rig.binding.session_key);
        assert_eq!(
            get(rig.loopback.port(), &host(&rig), &orch, Some(&token))
                .await
                .1,
            "named"
        );
        assert_ne!(
            get(rig.loopback.port(), &host(&rig), &other, Some(&token))
                .await
                .1,
            "named"
        );
    }

    #[tokio::test]
    async fn a_session_that_was_unbound_and_a_token_that_was_revoked_are_no_longer_answered() {
        let rig = rig().await;
        rig.loopback.register(EndpointKind::Exec, Arc::new(Echo));
        let token = rig.grant.token.expose().to_owned();
        let path = format!("/exec/{}", rig.binding.session_key);
        assert_eq!(
            get(rig.loopback.port(), &host(&rig), &path, Some(&token))
                .await
                .0,
            StatusCode::OK
        );
        rig.loopback.unbind(&rig.binding.session_key);
        assert_eq!(
            get(rig.loopback.port(), &host(&rig), &path, Some(&token))
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        rig.loopback.revoke(rig.grant.id);
        assert_eq!(
            get(rig.loopback.port(), &host(&rig), &path, Some(&token))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn tokens_and_keys_are_long_and_never_repeat() {
        let rig = rig().await;
        let second = rig.loopback.grant();
        assert_eq!(rig.grant.token.expose().len(), 64);
        assert_ne!(rig.grant.token.expose(), second.token.expose());
        assert!(orch_agent_contract::loopback::is_session_key(
            &rig.binding.session_key
        ));
        let other = rig.loopback.bind(
            rig.grant.id,
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            Tickets::default(),
        );
        assert_ne!(other.session_key, rig.binding.session_key);
    }
}
