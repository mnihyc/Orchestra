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
    /// Carrier plans beside the unchanged exec/llm/MCP signed strings.
    pub routes: BTreeMap<String, orch_link_contract::gateways::TunnelPlan>,
    /// Existing binding lifetimes retain/release the corresponding shared transports.
    pub route_leases: BTreeMap<String, Arc<orch_link::gateways::TunnelLease>>,
    /// For the model calls of a forwarded session.
    pub llm: Option<String>,
    /// For the connection to the executor.
    pub exec: Option<String>,
    /// For each tool server of the environment, by name.
    pub mcp: BTreeMap<String, String>,
}

/// Atomic MCP replacement and rollback, including its adjacent carrier metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpRoutes {
    /// Signed strings keyed by the existing MCP server name.
    pub tickets: BTreeMap<String, String>,
    /// Frozen plans keyed by that same server name.
    pub routes: BTreeMap<String, orch_link_contract::gateways::TunnelPlan>,
    /// Shared lifetime for each server's plan.
    pub leases: BTreeMap<String, Arc<orch_link::gateways::TunnelLease>>,
}

/// A session as the listener knows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionBinding {
    exchanges: Arc<Exchanges>,
    physical: Option<orch_link_contract::native_target::SessionTarget>,
    tool_revision: i64,
    tool_current: bool,
    tool_round: Option<Id>,
    admitted_members: Option<Vec<orch_link_contract::tool_sessions::ToolSessionMember>>,
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

/// Cloned bindings share cancellation, including requests that already hold an older ticket snapshot.
#[derive(Debug)]
struct Exchanges {
    stop: CancellationToken,
    members: BTreeMap<String, CancellationToken>,
}
impl PartialEq for Exchanges {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}
impl Eq for Exchanges {}
impl Exchanges {
    fn member(&self, member: &str) -> CancellationToken {
        self.members.get(member).unwrap_or(&self.stop).clone()
    }
    fn replace(&mut self, member: &str, present: bool) {
        if let Some(stop) = self.members.remove(member) {
            stop.cancel();
        }
        if present {
            self.members.insert(member.into(), self.stop.child_token());
        }
    }
}
impl SessionBinding {
    fn remote_member(&self, member: &str) -> bool {
        let ticket = match member {
            "exec" => self.tickets.exec.as_deref(),
            "llm" => self.tickets.llm.as_deref(),
            key => key
                .strip_prefix("mcp/")
                .and_then(|name| self.tickets.mcp.get(name))
                .map(String::as_str),
        };
        ticket.and_then(|ticket|orch_link_contract::tunnels::Ticket::parse(ticket).ok()).is_some_and(|ticket| {
            matches!(ticket.claims().src,orch_link_contract::tunnels::TicketSource::Machine(source) if source!=ticket.claims().dst)
        })
    }
    pub(crate) fn exchange_stop(&self, member: &str) -> CancellationToken {
        if self.physical.is_some() && !self.exchanges.members.contains_key(member) {
            let stop = CancellationToken::new();
            stop.cancel();
            return stop;
        }
        self.exchanges.member(member)
    }

    fn member_scope(&self, member: &str) -> Option<(String, Id, i64)> {
        let ticket = match member {
            "exec" => self.tickets.exec.as_deref(),
            "llm" => self.tickets.llm.as_deref(),
            name => name
                .strip_prefix("mcp/")
                .and_then(|name| self.tickets.mcp.get(name))
                .map(String::as_str),
        };
        if let Some(ticket) = ticket {
            let parsed = orch_link_contract::tunnels::Ticket::parse(ticket).ok()?;
            let scope = parsed.claims().tool_session.as_ref()?;
            // This only identifies an existing member to withdraw or retain under the separately
            // matched physical target. It never installs a route or verifies a remote ticket.
            return Some((scope.member.clone(), scope.nonce, scope.revision));
        }
        if matches!(member, "exec" | "llm") {
            return Some((member.into(), self.physical.as_ref()?.opening_id?, 0));
        }
        None
    }
}

/// Read only an authenticated command's stored ticket metadata to retain an unchanged member.
/// This does not verify a ticket or grant access; remote opens still ask the live API.
fn same_member(before: &str, after: &str) -> bool {
    if before == after {
        return true;
    }
    use orch_link_contract::tunnels::Ticket;
    let (Ok(before), Ok(after)) = (Ticket::parse(before), Ticket::parse(after)) else {
        return false;
    };
    match (&before.claims().tool_session, &after.claims().tool_session) {
        (Some(a), Some(b)) => {
            a.thread_id == b.thread_id
                && a.segment_id == b.segment_id
                && a.opening_id == b.opening_id
                && a.opening_epoch == b.opening_epoch
                && a.member == b.member
                && a.nonce == b.nonce
        }
        _ => false,
    }
}

fn configuration_matches(
    binding: &SessionBinding,
    action: orch_link_contract::native_target::SessionAction,
    expected: &[orch_link_contract::tool_sessions::ToolSessionMember],
) -> bool {
    use orch_link_contract::native_target::SessionAction;
    if !matches!(action, SessionAction::Model | SessionAction::Mcp) {
        return false;
    }
    let Some(current) = &binding.admitted_members else {
        return false;
    };
    let selected = |members: &[orch_link_contract::tool_sessions::ToolSessionMember]| {
        let mut result = BTreeMap::new();
        for member in members.iter().filter(|member| match action {
            SessionAction::Mcp => member.member.starts_with("mcp:"),
            SessionAction::Model => member.member == "llm",
            _ => false,
        }) {
            if result.insert(member.member.clone(), member.nonce).is_some() {
                return None;
            }
        }
        Some(result)
    };
    match (selected(current), selected(expected)) {
        (Some(current), Some(expected)) => {
            current == expected && (action != SessionAction::Model || current.len() == 1)
        }
        _ => false,
    }
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
    native_fences: Option<Arc<orch_link::native_fences::NativeFences>>,
    suspended: bool,
    hard_suspended: bool,
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
        state.sessions.retain(|_, session| {
            if session.grant == grant {
                session.exchanges.stop.cancel();
                false
            } else {
                true
            }
        });
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
        let stop = self.stop.child_token();
        let members = ["exec".into(), "llm".into(), "mcp/orch".into()]
            .into_iter()
            .chain(tickets.mcp.keys().map(|name| format!("mcp/{name}")))
            .map(|member| (member, stop.child_token()))
            .collect();
        let binding = Arc::new(SessionBinding {
            physical: None,
            tool_revision: 0,
            tool_current: true,
            tool_round: None,
            admitted_members: None,
            exchanges: Arc::new(Exchanges { stop, members }),
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

    /// The binding of one admitted physical opening, separate from the private local key.
    pub fn bind_native(
        &self,
        grant: u64,
        provider: Id,
        agent: Id,
        target: &orch_link_contract::native_target::SessionTarget,
        tickets: Tickets,
    ) -> Option<Arc<SessionBinding>> {
        if !target.coherent() || target.opening_id.is_none() {
            return None;
        }
        let binding = self.bind(
            grant,
            target.thread_id,
            target.segment_id?,
            provider,
            agent,
            tickets,
        );
        let mut physical = binding.as_ref().clone();
        physical.physical = Some(target.clone());
        let mut state = self.shared.state();
        physical.tool_current = !state.suspended;
        physical.tool_round = state
            .native_fences
            .as_ref()
            .map(|fences| fences.tool_control().0);
        let physical = Arc::new(physical);
        state
            .sessions
            .insert(binding.session_key.clone(), Arc::clone(&physical));
        Some(physical)
    }

    /// Notices only withdraw existing members; installing a changed route still requires its native command.
    pub fn apply_tool_update(&self, update: &orch_link_contract::tool_sessions::ToolSessionUpdate) {
        self.change_tool_update(update, true, None);
    }
    /// A stored snapshot still restricts a queued command but cannot revalidate a lost control connection.
    pub fn restrict_tool_update(
        &self,
        update: &orch_link_contract::tool_sessions::ToolSessionUpdate,
    ) {
        self.change_tool_update(update, false, None);
    }
    pub fn revalidate_tool_update(
        &self,
        update: &orch_link_contract::tool_sessions::ToolSessionUpdate,
        round: Id,
    ) {
        self.change_tool_update(update, true, Some(round));
    }
    pub fn native_fences(&self, fences: Arc<orch_link::native_fences::NativeFences>) {
        self.shared.state().native_fences = Some(fences);
    }
    fn change_tool_update(
        &self,
        update: &orch_link_contract::tool_sessions::ToolSessionUpdate,
        fresh: bool,
        expected_round: Option<Id>,
    ) {
        let mut state = self.shared.state();
        let control = state
            .native_fences
            .as_ref()
            .map(|fences| fences.tool_control());
        if let Some(expected) = expected_round
            && control != Some((expected, true))
        {
            return;
        }
        let keys = state
            .sessions
            .iter()
            .filter(|(_, binding)| binding.thread_id == update.thread_id)
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            let Some(current) = state.sessions.get(&key).cloned() else {
                continue;
            };
            let Some(physical) = &current.physical else {
                continue;
            };
            let Some(epoch) = physical.opening_epoch else {
                continue;
            };
            if epoch > update.opening_epoch
                || epoch == update.opening_epoch && current.tool_revision > update.revision
            {
                continue;
            }
            if epoch < update.opening_epoch
                || !update.live
                || physical.opening_id != update.opening_id
                || physical.segment_id != Some(update.segment_id)
            {
                current.exchanges.stop.cancel();
                state.sessions.remove(&key);
                state.reaches.remove(&key);
                continue;
            }
            let mut changed = current.as_ref().clone();
            let mut admitted = update.members.clone();
            if current.tool_revision == update.revision
                && let Some(previous) = &current.admitted_members
            {
                admitted.retain(|member| previous.contains(member));
            }
            changed.admitted_members = Some(admitted.clone());
            let mut exchanges = Exchanges {
                stop: current.exchanges.stop.clone(),
                members: current.exchanges.members.clone(),
            };
            for member in current
                .exchanges
                .members
                .keys()
                .filter(|key| key.as_str() != "mcp/orch")
            {
                let keep = current
                    .member_scope(member)
                    .is_some_and(|(name, nonce, revision)| {
                        revision > update.revision
                            || admitted
                                .iter()
                                .any(|member| member.member == name && member.nonce == nonce)
                    });
                if keep {
                    if fresh && !state.suspended && exchanges.member(member).is_cancelled() {
                        exchanges.replace(member, true);
                    }
                    continue;
                }
                exchanges.replace(member, false);
                match member.as_str() {
                    "exec" => {
                        changed.tickets.exec = None;
                    }
                    "llm" => {
                        changed.tickets.llm = None;
                    }
                    name => {
                        if let Some(name) = name.strip_prefix("mcp/") {
                            changed.tickets.mcp.remove(name);
                        }
                    }
                }
                changed.tickets.routes.remove(member);
                changed.tickets.route_leases.remove(member);
            }
            changed.tool_revision = update.revision;
            changed.tool_current |= fresh;
            if fresh {
                changed.tool_round = control.map(|control| control.0);
            }
            changed.exchanges = Arc::new(exchanges);
            state.sessions.insert(key, Arc::new(changed));
        }
    }

    /// Pause local exchanges while retaining the native conversation and unchanged route facts.
    pub fn tool_control(&self, available: bool) {
        let mut state = self.shared.state();
        if available && !state.suspended || !available && state.hard_suspended {
            return;
        }
        state.suspended = !available;
        state.hard_suspended = !available;
        let round = state
            .native_fences
            .as_ref()
            .map(|fences| fences.tool_control().0);
        for binding in state.sessions.values_mut() {
            if !available {
                for stop in binding.exchanges.members.values() {
                    stop.cancel();
                }
                if binding.physical.is_some() {
                    let mut changed = binding.as_ref().clone();
                    changed.tool_current = false;
                    *binding = Arc::new(changed);
                }
            } else {
                if !binding.tool_current
                    || binding.physical.is_some() && binding.tool_round != round
                {
                    continue;
                }
                let mut changed = binding.as_ref().clone();
                changed.exchanges = Arc::new(Exchanges {
                    stop: binding.exchanges.stop.clone(),
                    members: binding
                        .exchanges
                        .members
                        .keys()
                        .map(|member| {
                            let previous = binding.exchanges.member(member);
                            (
                                member.clone(),
                                if previous.is_cancelled() {
                                    binding.exchanges.stop.child_token()
                                } else {
                                    previous
                                },
                            )
                        })
                        .collect(),
                });
                *binding = Arc::new(changed);
            }
        }
    }

    /// A primary loss pauses local work; independent remote carriers retain their own live admission.
    pub fn refresh_tool_control(&self) {
        let mut state = self.shared.state();
        state.suspended = true;
        for binding in state.sessions.values_mut() {
            for (member, stop) in &binding.exchanges.members {
                if !binding.remote_member(member) {
                    stop.cancel();
                }
            }
            if binding.physical.is_some() {
                let mut changed = binding.as_ref().clone();
                changed.tool_current = false;
                *binding = Arc::new(changed);
            }
        }
    }

    /// Changes MCP routes at the same key and ends exchanges of withdrawn or replaced members.
    pub fn replace_mcp(
        &self,
        session_key: &str,
        tickets: BTreeMap<String, String>,
    ) -> Option<BTreeMap<String, String>> {
        self.replace_mcp_routed(
            session_key,
            McpRoutes {
                tickets,
                ..McpRoutes::default()
            },
        )
        .map(|previous| previous.tickets)
    }

    /// Replaces signed tickets and plans together, preserving unchanged member exchanges only.
    pub fn replace_mcp_routed(
        &self,
        session_key: &str,
        replacement: McpRoutes,
    ) -> Option<McpRoutes> {
        self.replace_mcp_inner(session_key, replacement, None)
    }
    pub fn replace_mcp_admitted(
        &self,
        session_key: &str,
        replacement: McpRoutes,
        members: &[orch_link_contract::tool_sessions::ToolSessionMember],
    ) -> Option<McpRoutes> {
        self.replace_mcp_inner(session_key, replacement, Some(members))
    }
    fn replace_mcp_inner(
        &self,
        session_key: &str,
        replacement: McpRoutes,
        expected: Option<&[orch_link_contract::tool_sessions::ToolSessionMember]>,
    ) -> Option<McpRoutes> {
        let mut state = self.shared.state();
        let current = state.sessions.get(session_key)?.as_ref().clone();
        if expected.is_some_and(|expected| {
            !configuration_matches(
                &current,
                orch_link_contract::native_target::SessionAction::Mcp,
                expected,
            )
        }) {
            return None;
        }
        let mut exchanges = Exchanges {
            stop: current.exchanges.stop.clone(),
            members: current.exchanges.members.clone(),
        };
        for (name, before) in &current.tickets.mcp {
            if !replacement
                .tickets
                .get(name)
                .is_some_and(|after| same_member(before, after))
            {
                exchanges.replace(
                    &format!("mcp/{name}"),
                    replacement.tickets.contains_key(name),
                );
            }
        }
        for name in replacement
            .tickets
            .keys()
            .filter(|name| !current.tickets.mcp.contains_key(*name))
        {
            exchanges.replace(&format!("mcp/{name}"), true);
        }
        let mut changed = current;
        changed.exchanges = Arc::new(exchanges);
        let previous_tickets = std::mem::replace(&mut changed.tickets.mcp, replacement.tickets);
        let mut previous = McpRoutes {
            tickets: previous_tickets,
            ..McpRoutes::default()
        };
        let keys = changed
            .tickets
            .routes
            .keys()
            .filter_map(|key| key.strip_prefix("mcp/").map(str::to_owned))
            .collect::<Vec<_>>();
        for name in keys {
            let key = format!("mcp/{name}");
            if let Some(route) = changed.tickets.routes.remove(&key) {
                previous.routes.insert(name.clone(), route);
            }
            if let Some(lease) = changed.tickets.route_leases.remove(&key) {
                previous.leases.insert(name, lease);
            }
        }
        changed.tickets.routes.extend(
            replacement
                .routes
                .into_iter()
                .map(|(name, route)| (format!("mcp/{name}"), route)),
        );
        changed.tickets.route_leases.extend(
            replacement
                .leases
                .into_iter()
                .map(|(name, lease)| (format!("mcp/{name}"), lease)),
        );
        state
            .sessions
            .insert(session_key.to_owned(), Arc::new(changed));
        Some(previous)
    }

    /// Rotate the admitted model lease without changing the native process's addresses.
    /// A replaced member ends its old calls; an unchanged admitted member keeps its exchanges.
    pub fn replace_llm(&self, session_key: &str, ticket: Option<String>) -> Option<Option<String>> {
        self.replace_llm_inner(session_key, ticket, None)
    }
    pub fn replace_llm_admitted(
        &self,
        session_key: &str,
        ticket: Option<String>,
        members: &[orch_link_contract::tool_sessions::ToolSessionMember],
    ) -> Option<Option<String>> {
        self.replace_llm_inner(session_key, ticket, Some(members))
    }
    fn replace_llm_inner(
        &self,
        session_key: &str,
        ticket: Option<String>,
        expected: Option<&[orch_link_contract::tool_sessions::ToolSessionMember]>,
    ) -> Option<Option<String>> {
        let mut state = self.shared.state();
        let mut changed = state.sessions.get(session_key)?.as_ref().clone();
        if expected.is_some_and(|expected| {
            !configuration_matches(
                &changed,
                orch_link_contract::native_target::SessionAction::Model,
                expected,
            )
        }) {
            return None;
        }
        let unchanged = match (&changed.tickets.llm, &ticket) {
            (None, None) => true,
            (Some(before), Some(after)) => same_member(before, after),
            _ => false,
        };
        if !unchanged {
            let mut exchanges = Exchanges {
                stop: changed.exchanges.stop.clone(),
                members: changed.exchanges.members.clone(),
            };
            exchanges.replace("llm", true);
            changed.exchanges = Arc::new(exchanges);
        }
        let previous = std::mem::replace(&mut changed.tickets.llm, ticket);
        state
            .sessions
            .insert(session_key.to_owned(), Arc::new(changed));
        Some(previous)
    }

    pub fn configuration_current(
        &self,
        session_key: &str,
        action: orch_link_contract::native_target::SessionAction,
        expected: &[orch_link_contract::tool_sessions::ToolSessionMember],
    ) -> bool {
        self.shared
            .state()
            .sessions
            .get(session_key)
            .is_some_and(|binding| configuration_matches(binding, action, expected))
    }

    /// Forgets a session: its addresses are answered as if they never were.
    pub fn unbind(&self, session_key: &str) {
        let mut state = self.shared.state();
        if let Some(session) = state.sessions.remove(session_key) {
            session.exchanges.stop.cancel();
        }
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
        if state.suspended {
            return text(
                StatusCode::SERVICE_UNAVAILABLE,
                "current tool session control is unavailable",
            );
        }
        let session = state
            .sessions
            .get(&key)
            .filter(|session| session.grant == grant)
            .cloned();
        if let Some(session) = &session
            && session.physical.is_some()
            && state.native_fences.as_ref().is_some_and(|fences| {
                fences.tool_control() != (session.tool_round.unwrap_or(Id::nil()), true)
            })
        {
            return text(
                StatusCode::SERVICE_UNAVAILABLE,
                "the current native scope is being checked",
            );
        }
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
    if !session.tool_current {
        return text(
            StatusCode::SERVICE_UNAVAILABLE,
            "the current native scope is being checked",
        );
    }
    match endpoint {
        Some(endpoint) => {
            let member = match &route {
                Route::Llm { .. } => "llm".into(),
                Route::Exec { .. } => "exec".into(),
                Route::Mcp { server, .. } if server == "exec" => "exec".into(),
                Route::Mcp { server, .. } => format!("mcp/{server}"),
            };
            let stop = session.exchange_stop(&member);
            if session.physical.is_some()
                && member != "mcp/orch"
                && session.admitted_members.as_ref().is_some_and(|admitted| {
                    !session
                        .member_scope(&member)
                        .is_some_and(|(name, nonce, _)| {
                            admitted
                                .iter()
                                .any(|member| member.member == name && member.nonce == nonce)
                        })
                })
            {
                return text(StatusCode::GONE, "this session member was withdrawn");
            }
            let response = tokio::select! {
                biased;
                () = stop.cancelled() => return text(StatusCode::GONE, "this session member was withdrawn"),
                response = endpoint.serve(request, session, route) => response,
            };
            if response.status() == StatusCode::SWITCHING_PROTOCOLS {
                return response;
            }
            let (parts, body) = response.into_parts();
            let stream = futures::stream::unfold(Some((body, stop)), |state| async move {
                let (mut body, stop) = state?;
                let frame = tokio::select! {
                    biased;
                    () = stop.cancelled() => return Some((Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted, "the session member was withdrawn",
                    )), None)),
                    frame = body.frame() => frame,
                }?;
                Some((frame, Some((body, stop))))
            });
            Response::from_parts(parts, http_body_util::StreamBody::new(stream).boxed())
        }
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

    struct HeldRequest(Arc<tokio::sync::Notify>);
    #[async_trait]
    impl Endpoint for HeldRequest {
        async fn serve(
            &self,
            _: Request<Incoming>,
            _: Arc<SessionBinding>,
            _: Route,
        ) -> Response<Body> {
            self.0.notify_one();
            std::future::pending().await
        }
    }

    fn held_request(
        rig: &Rig,
        server: &str,
    ) -> (
        Arc<tokio::sync::Notify>,
        tokio::task::JoinHandle<(StatusCode, String)>,
    ) {
        let entered = Arc::new(tokio::sync::Notify::new());
        rig.loopback.register(
            EndpointKind::Mcp(server.into()),
            Arc::new(HeldRequest(entered.clone())),
        );
        let port = rig.loopback.port();
        let host = host(rig);
        let token = rig.grant.token.expose().to_owned();
        let path = format!("/mcp/{}/{server}", rig.binding.session_key);
        (
            entered,
            tokio::spawn(async move { get(port, &host, &path, Some(&token)).await }),
        )
    }

    #[tokio::test]
    async fn retiring_a_session_or_its_process_ends_its_already_waiting_local_request() {
        for process in [false, true] {
            let rig = rig().await;
            let (entered, response) = held_request(&rig, "orch");
            tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
                .await
                .unwrap();
            if process {
                rig.loopback.revoke(rig.grant.id);
            } else {
                rig.loopback.unbind(&rig.binding.session_key);
            }
            let (status, _) = tokio::time::timeout(std::time::Duration::from_secs(10), response)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(status, StatusCode::GONE);
        }
    }

    #[tokio::test]
    async fn losing_live_tool_control_ends_local_work_without_forgetting_the_native_binding() {
        let rig = rig().await;
        let (entered, response) = held_request(&rig, "orch");
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
            .await
            .unwrap();
        rig.loopback.tool_control(false);
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(10), response)
                .await
                .unwrap()
                .unwrap()
                .0,
            StatusCode::GONE
        );
        assert!(rig.loopback.session(&rig.binding.session_key).is_some());
        rig.loopback
            .register(EndpointKind::Mcp("orch".into()), Arc::new(Echo));
        let path = format!("/mcp/{}/orch", rig.binding.session_key);
        assert_eq!(
            get(
                rig.loopback.port(),
                &host(&rig),
                &path,
                Some(rig.grant.token.expose())
            )
            .await
            .0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        rig.loopback.tool_control(true);
        assert_eq!(
            get(
                rig.loopback.port(),
                &host(&rig),
                &path,
                Some(rig.grant.token.expose())
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn a_committed_member_withdrawal_ends_local_exec_before_its_native_reload_and_keeps_the_conversation()
     {
        use orch_link_contract::native_target::SessionTarget;
        use orch_link_contract::tool_sessions::ToolSessionUpdate;
        let mut rig = rig().await;
        let target = SessionTarget {
            thread_id: rig.binding.thread_id,
            segment_id: Some(rig.binding.segment_id),
            opening_id: Some(new_id()),
            opening_epoch: Some(1),
        };
        rig.binding = rig
            .loopback
            .bind_native(
                rig.grant.id,
                rig.binding.provider_id,
                rig.binding.agent_id,
                &target,
                Tickets::default(),
            )
            .unwrap();
        let (entered, response) = held_request(&rig, "exec");
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
            .await
            .unwrap();
        rig.loopback.apply_tool_update(&ToolSessionUpdate {
            machine_id: new_id(),
            thread_id: target.thread_id,
            segment_id: target.segment_id.unwrap(),
            opening_id: target.opening_id,
            opening_epoch: 1,
            revision: 2,
            live: true,
            members: vec![],
        });
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(10), response)
                .await
                .unwrap()
                .unwrap()
                .0,
            StatusCode::GONE
        );
        assert!(rig.loopback.session(&rig.binding.session_key).is_some());
        rig.loopback
            .register(EndpointKind::Mcp("exec".into()), Arc::new(Echo));
        let path = format!("/mcp/{}/exec", rig.binding.session_key);
        assert_eq!(
            get(
                rig.loopback.port(),
                &host(&rig),
                &path,
                Some(rig.grant.token.expose())
            )
            .await
            .0,
            StatusCode::GONE
        );
        rig.loopback
            .register(EndpointKind::Mcp("orch".into()), Arc::new(Echo));
        let independent = format!("/mcp/{}/orch", rig.binding.session_key);
        assert_eq!(
            get(
                rig.loopback.port(),
                &host(&rig),
                &independent,
                Some(rig.grant.token.expose())
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn withdrawing_one_local_tool_ends_its_waiting_request_and_keeps_an_independent_member() {
        let rig = rig().await;
        rig.loopback
            .replace_mcp(
                &rig.binding.session_key,
                BTreeMap::from([
                    ("files".into(), "synthetic-files".into()),
                    ("notes".into(), "synthetic-notes".into()),
                ]),
            )
            .unwrap();
        rig.loopback
            .register(EndpointKind::Mcp("notes".into()), Arc::new(Echo));
        let (entered, response) = held_request(&rig, "files");
        tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
            .await
            .unwrap();
        rig.loopback
            .replace_mcp(
                &rig.binding.session_key,
                BTreeMap::from([("notes".into(), "synthetic-notes".into())]),
            )
            .unwrap();
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(10), response)
                .await
                .unwrap()
                .unwrap()
                .0,
            StatusCode::GONE
        );
        let path = format!("/mcp/{}/notes", rig.binding.session_key);
        assert_eq!(
            get(
                rig.loopback.port(),
                &host(&rig),
                &path,
                Some(rig.grant.token.expose())
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn retiring_a_session_ends_a_local_response_that_has_already_sent_its_headers() {
        struct Streaming;
        #[async_trait]
        impl Endpoint for Streaming {
            async fn serve(
                &self,
                _: Request<Incoming>,
                _: Arc<SessionBinding>,
                _: Route,
            ) -> Response<Body> {
                use futures::StreamExt;
                let prefix = futures::stream::once(async {
                    Ok::<_, std::io::Error>(hyper::body::Frame::data(Bytes::from_static(b"prefix")))
                });
                let body = prefix.chain(futures::stream::pending());
                Response::new(http_body_util::StreamBody::new(body).boxed())
            }
        }
        let rig = rig().await;
        rig.loopback
            .register(EndpointKind::Exec, Arc::new(Streaming));
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", rig.loopback.port()))
            .await
            .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let request = Request::builder()
            .uri(format!("/exec/{}", rig.binding.session_key))
            .header("host", host(&rig))
            .header(
                "authorization",
                format!("Bearer {}", rig.grant.token.expose()),
            )
            .body(Full::new(Bytes::new()))
            .unwrap();
        let mut response = sender.send_request(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .body_mut()
                .frame()
                .await
                .unwrap()
                .unwrap()
                .into_data()
                .unwrap(),
            "prefix"
        );
        rig.loopback.unbind(&rig.binding.session_key);
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_secs(10),
                response.into_body().collect()
            )
            .await
            .unwrap()
            .is_err()
        );
    }
}
