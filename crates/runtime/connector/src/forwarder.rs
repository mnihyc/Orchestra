//! Lease, model, allowance and path checks, endpoint choice by protocol, the key presented as the endpoint says with its headers and query, optional translation and upstream streaming.
//!
//! A model call of a harness that runs beside its tools reaches the connector's listener under the key of its
//! session (`/llm/<key>/…`), with the token of its process. What the forwarder does with it is the same
//! whether the key is kept on this machine or the call came through a tunnel from another, so that the lease
//! and the allowance of models hold even when harness and key share a machine. The steps, in this order, each
//! refused in the error shape of the protocol the call is in, with a code of the forwarder's own:
//!
//! 1. The lease the call runs under is open (`lease_closed`, 403): the one last opened for the session's
//!    thread, or, through a tunnel, the one its ticket names.
//! 2. The method and the path are on the protocol's list (`path_not_allowed`, 404). A harness is given a
//!    base address that ends in the protocol's own prefix (`/v1` for the Responses format, none for the
//!    Messages format); what follows that prefix, or the path as it is, is checked against the list exactly.
//! 3. The body is read, up to the largest request the machine forwards (`request_too_large`, 413), and the
//!    model it names is one of the lease's, compared as the string it is (`model_not_allowed`, 403, with a
//!    `lease.denied` for the system). A request for the list of models is answered here, with the lease's
//!    models and no others, and never goes upstream.
//!    A body that holds a member the machine's privacy rules refuse is refused whole (`request_refused`, 403), so
//!    that a program that was told not to upload what it records and does it anyway sends nothing ([`Privacy`]).
//! 4. The allowance of the lease is not used up (`cap_reached`, 403, with a `lease.denied`).
//! 5. The endpoint is the one of the provider's upstream that speaks the call's protocol
//!    (`protocol_not_served`, 404, when none does: no translator is built in).
//! 6. What the harness presented is removed (`Authorization`, `x-api-key`, `Cookie`, the hop-by-hop headers,
//!    any header the endpoint names, and the headers that name a client by who it is, see [`Privacy`]), the key is
//!    added the way the endpoint says, then its headers, its secret headers and its query; the harness asks for
//!    the answer uncompressed, so that what it used can be read from it.
//! 7. The request is sent and the answer is handed on while it arrives, status and headers as the upstream
//!    gave them, an error among them. What it used is read as it passes ([`crate::usage`]); the lease is kept up to
//!    date piece by piece and a `lease.usage` is reported when the answer ends, whole or cut.
//!
//! An upstream that cannot be reached is `upstream_unreachable` (502) or `upstream_timeout` (504), with a
//! `Retry-After`, so that the harness waits the way it does for any busy service. An upstream that refuses
//! the key with authentication evidence makes the provider `error` for the system to see, and `ready` again
//! when it answers well. A request refused by a model's filter does not refuse the shared credential.
//!
//! A call whose provider is not on this machine goes to the machine that has it through an `llm` tunnel
//! (one HTTP exchange for each tunnel, opened with the ticket the session was given); the machine that
//! serves the tunnel runs the same steps ([`LlmTunnels`]).

use std::convert::Infallible;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::header::{
    ACCEPT_ENCODING, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH, HOST, HeaderMap, HeaderName,
    HeaderValue, RETRY_AFTER, TRANSFER_ENCODING,
};
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orch_agent_contract::loopback::Route;
use orch_link::tunnels::channel::IncomingTunnel;
use orch_link_contract::events::{Event, ProviderStatus};
use orch_link_contract::streams::Stream;
use orch_link_contract::tunnels::{OpenErrorCode, TunnelKind};
use orch_model_contract::endpoint::AuthorizeError;
use orch_model_contract::protocol::WireProtocol;
use orch_model_contract::translator::{self, Route as Translation};
use orch_model_contract::usage::{ObservedUsage, UsageObserver};
use orch_types::Id;
use url::Url;

use crate::credentials::SecretStore;
use crate::events::Publisher;
use crate::leases::{Lease, Leases, Meter};
use crate::loopback::{Body, Endpoint, SessionBinding, text};
use crate::roles::Services;
use crate::roles::provider::{EndpointEntry, ProviderEntry, Providers};
use crate::tunnels::TunnelServer;
use crate::upstream::{
    End, IDLE_TIMEOUT, Relay, Upstream, UpstreamError, UpstreamRequest, Watcher,
};
use crate::usage::{lease_denied, lease_usage};

/// How long a harness is told to wait before it asks again when the forwarder could not reach the
/// upstream, or the provider is not there: the first pause of the harnesses' own backoff.
pub const RETRY_AFTER_SECS: u32 = 5;
/// The host a request is given on the far side of a tunnel: the exchange is one HTTP exchange on a stream,
/// and the name only has to be a name.
const TUNNEL_HOST: &str = "llm.orch.internal";

/// The refusal codes of the forwarder, written in the error of the protocol the call is in.
pub mod code {
    /// The lease the call runs under is closed, or there is none.
    pub const LEASE_CLOSED: &str = "lease_closed";
    /// The model the call names is not one of the lease's.
    pub const MODEL_NOT_ALLOWED: &str = "model_not_allowed";
    /// The lease's allowance is used up.
    pub const CAP_REACHED: &str = "cap_reached";
    /// The protocol does not let that method and path through.
    pub const PATH_NOT_ALLOWED: &str = "path_not_allowed";
    /// No endpoint of the provider speaks the call's protocol.
    pub const PROTOCOL_NOT_SERVED: &str = "protocol_not_served";
    /// The body is larger than the machine forwards.
    pub const REQUEST_TOO_LARGE: &str = "request_too_large";
    /// The body carries a member that the privacy rules do not let leave the machine.
    pub const REQUEST_REFUSED: &str = "request_refused";
    /// The body is encoded, and the model it names cannot be read.
    pub const UNSUPPORTED_ENCODING: &str = "unsupported_encoding";
    /// The provider is not started here, or has no key.
    pub const PROVIDER_UNAVAILABLE: &str = "provider_unavailable";
    /// The selected key presenter could not supply a usable credential for this call.
    pub const CREDENTIAL_UNAVAILABLE: &str = "credential_unavailable";
    /// The upstream could not be reached.
    pub const UPSTREAM_UNREACHABLE: &str = "upstream_unreachable";
    /// The upstream took too long.
    pub const UPSTREAM_TIMEOUT: &str = "upstream_timeout";
}

/// Makes the answer to a harness's request for the list of models: exactly the lease's, in the shape of the
/// protocol's list.
pub type ModelsList = dyn Fn(&[String]) -> Response<Bytes> + Send + Sync;

/// A wire protocol as the forwarder serves it.
#[derive(Clone)]
pub struct ProtocolEntry {
    protocol: Arc<dyn WireProtocol>,
    harness_prefix: &'static str,
    models: Arc<ModelsList>,
}

impl std::fmt::Debug for ProtocolEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolEntry")
            .field("protocol", &self.protocol.name())
            .field("harness_prefix", &self.harness_prefix)
            .finish_non_exhaustive()
    }
}

impl ProtocolEntry {
    /// A protocol, the prefix the base address a harness is given ends in, and how the list of a lease's
    /// models is written in it.
    pub fn new(
        protocol: impl WireProtocol,
        harness_prefix: &'static str,
        models: impl Fn(&[String]) -> Response<Bytes> + Send + Sync + 'static,
    ) -> Self {
        Self {
            protocol: Arc::new(protocol),
            harness_prefix,
            models: Arc::new(models),
        }
    }

    /// The protocol.
    #[must_use]
    pub fn protocol(&self) -> &Arc<dyn WireProtocol> {
        &self.protocol
    }

    /// Its name.
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.protocol.name()
    }

    /// The path of a harness's request below its session, as the protocol's list knows it, or `None` when
    /// the protocol does not let the request through. The path is taken after the prefix of the base address
    /// the harness was given, or as it is.
    #[must_use]
    pub fn resolve(&self, method: &Method, below_session: &str) -> Option<String> {
        let after_prefix = (!self.harness_prefix.is_empty())
            .then(|| below_session.strip_prefix(self.harness_prefix))
            .flatten()
            .filter(|rest| rest.starts_with('/'));
        [after_prefix, Some(below_session)]
            .into_iter()
            .flatten()
            .find(|path| self.protocol.allowed(method, path))
            .map(str::to_owned)
    }
}

/// The wire protocols a connector forwards.
#[derive(Clone, Default)]
pub struct Protocols {
    entries: Vec<ProtocolEntry>,
    translators: Vec<Arc<dyn translator::ProtocolTranslator>>,
    privacy: Privacy,
}

impl std::fmt::Debug for Protocols {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Protocols")
            .field("entries", &self.entries)
            .field("translators", &self.translator_ends())
            .finish()
    }
}

impl Protocols {
    /// No protocol.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a protocol.
    #[must_use]
    pub fn with(mut self, entry: ProtocolEntry) -> Self {
        self.entries.push(entry);
        self
    }

    /// Holds every call to these privacy rules.
    #[must_use]
    pub fn with_privacy(mut self, privacy: Privacy) -> Self {
        self.privacy = privacy;
        self
    }

    /// The privacy rules every call is held to.
    #[must_use]
    pub fn privacy(&self) -> &Privacy {
        &self.privacy
    }

    /// Adds the external translators registered in this runtime, in registration order.
    #[must_use]
    pub fn with_translators(
        mut self,
        translators: Vec<Arc<dyn translator::ProtocolTranslator>>,
    ) -> Self {
        self.translators = translators;
        self
    }

    /// The format pairs the registered translators implement.
    #[must_use]
    pub fn translator_ends(&self) -> Vec<translator::Ends<'_>> {
        self.translators
            .iter()
            .map(|translator| (translator.from(), translator.to()))
            .collect()
    }

    /// How a request can reach these endpoint protocols.
    #[must_use]
    pub fn route(&self, from: &str, endpoints: &[&str]) -> Translation {
        translator::route(from, endpoints, &self.translator_ends())
    }

    /// The protocol of this name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ProtocolEntry> {
        self.entries.iter().find(|entry| entry.name() == name)
    }

    /// The names of the protocols, in the order they were added.
    #[must_use]
    pub fn names(&self) -> Vec<&'static str> {
        self.entries.iter().map(ProtocolEntry::name).collect()
    }

    /// The protocol a request is in, told by what it asks for: the first whose list has it.
    #[must_use]
    pub fn of_request(&self, method: &Method, below_session: &str) -> Option<&ProtocolEntry> {
        self.entries
            .iter()
            .find(|entry| entry.resolve(method, below_session).is_some())
    }
}

/// A call the forwarder refuses.
struct Refusal {
    status: StatusCode,
    code: &'static str,
    message: String,
    retry_after: Option<u32>,
}

impl Refusal {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            retry_after: None,
        }
    }

    fn later(mut self) -> Self {
        self.retry_after = Some(RETRY_AFTER_SECS);
        self
    }
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes)
        .map_err(|never: Infallible| match never {})
        .boxed()
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    orch_model_contract::endpoint::HOP_BY_HOP.contains(&name.as_str())
}

/// What leaves with a model call and what does not, whichever harness made it.
///
/// A program may add to its requests what Orch never asked for: headers that name the client and the person's
/// installation, and members of the body that carry a record of the session to its vendor. Orch sets the program
/// not to, and the forwarder holds to it: it drops the headers that name the client and refuses the body that
/// holds one of the members, whole, with nothing sent. The rules are data of the programs' manifests, joined at the
/// composition of the connector; an empty set lets everything through that the steps above let through.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Privacy {
    strip_headers: Vec<String>,
    refuse_members: Vec<String>,
}

impl Privacy {
    /// Rules that drop the headers named by `strip_headers` (a name, or a prefix ending in `*`, compared without
    /// case) and refuse a body that has one of `refuse_members` as a member of its top level.
    #[must_use]
    pub fn new(strip_headers: &[String], refuse_members: &[String]) -> Self {
        Self {
            strip_headers: strip_headers
                .iter()
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| !name.is_empty())
                .collect(),
            refuse_members: refuse_members
                .iter()
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .collect(),
        }
    }

    /// Whether the header is one the rules drop.
    #[must_use]
    pub fn strips(&self, name: &HeaderName) -> bool {
        let name = name.as_str();
        self.strip_headers
            .iter()
            .any(|rule| match rule.strip_suffix('*') {
                Some(prefix) => name.starts_with(prefix),
                None => name == rule,
            })
    }

    /// The member a body holds that the rules refuse, if it is a JSON object that holds one. A body that is no
    /// object, or no JSON, holds none: what the model interface makes of it is for the protocol's own checks.
    #[must_use]
    pub fn refused_member(&self, body: &[u8]) -> Option<String> {
        if self.refuse_members.is_empty() {
            return None;
        }
        let members: std::collections::HashMap<String, &serde_json::value::RawValue> =
            serde_json::from_slice(body).ok()?;
        self.refuse_members
            .iter()
            .find(|member| members.contains_key(member.as_str()))
            .cloned()
    }
}

/// The forwarder: everything a call needs, shared by the listener, the tunnels and the provider role.
pub struct Forwarder {
    protocols: Protocols,
    providers: Arc<Providers>,
    leases: Arc<Leases>,
    upstream: Upstream,
    services: Arc<Services>,
    store: Arc<SecretStore>,
    max_request_bytes: u64,
}

impl std::fmt::Debug for Forwarder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forwarder").finish_non_exhaustive()
    }
}

/// What a call is under, and where the call came from.
enum Origin<'a> {
    /// A harness beside this machine's listener, under the key of its session.
    Session(&'a SessionBinding),
    /// Another machine, through a tunnel, under the lease its ticket names.
    Ticket { lease_id: Id, thread_id: Option<Id> },
}

enum AuthorizationError {
    Endpoint(AuthorizeError),
    Presentation(orch_provider_contract::SourceError),
}

impl Forwarder {
    /// A forwarder over the services of a connector.
    #[must_use]
    pub fn new(
        services: &Arc<Services>,
        protocols: Protocols,
        providers: Arc<Providers>,
        leases: Arc<Leases>,
        store: Arc<SecretStore>,
        upstream: Upstream,
    ) -> Arc<Self> {
        Arc::new(Self {
            protocols,
            providers,
            leases,
            upstream,
            max_request_bytes: services.config.provider.max_request_bytes,
            services: Arc::clone(services),
            store,
        })
    }

    /// The protocols it serves.
    #[must_use]
    pub fn protocols(&self) -> &Protocols {
        &self.protocols
    }

    /// The client to the upstreams.
    #[must_use]
    pub fn upstream(&self) -> &Upstream {
        &self.upstream
    }

    /// The sealed store the keys kept on this machine are in.
    #[must_use]
    pub fn store(&self) -> &Arc<SecretStore> {
        &self.store
    }

    async fn authorize(
        &self,
        provider: &ProviderEntry,
        endpoint: &EndpointEntry,
        entry: Option<&ProtocolEntry>,
        headers: &mut HeaderMap,
        credential: Option<&orch_model_contract::credentials::Credential>,
    ) -> Result<Vec<(String, String)>, AuthorizationError> {
        if let Some(presenter) = provider.presenter() {
            let mut options = endpoint.options().clone();
            if let Some(name) = options
                .auth_header
                .as_ref()
                .and_then(|name| HeaderName::from_bytes(name.as_bytes()).ok())
            {
                headers.remove(name);
            }
            options.auth = Some(orch_model_contract::endpoint::AuthMode::None);
            options.auth_header = None;
            let query = match entry {
                Some(entry) => {
                    entry
                        .protocol
                        .authorize(headers, None, &options, provider.secrets())
                }
                None => orch_model_contract::endpoint::authorize(
                    headers,
                    None,
                    &options,
                    provider.secrets(),
                    orch_model_contract::endpoint::AuthMode::None,
                ),
            }
            .map_err(AuthorizationError::Endpoint)?;
            let presented = presenter
                .headers(endpoint, credential)
                .await
                .map_err(AuthorizationError::Presentation)?;
            for (name, value) in &presented {
                headers.insert(name.clone(), value.clone());
            }
            Ok(query)
        } else {
            entry
                .map_or_else(
                    || Ok(Vec::new()),
                    |entry| {
                        entry.protocol.authorize(
                            headers,
                            credential,
                            endpoint.options(),
                            provider.secrets(),
                        )
                    },
                )
                .map_err(AuthorizationError::Endpoint)
        }
    }

    fn refuse(&self, entry: Option<&ProtocolEntry>, refusal: Refusal) -> Response<Body> {
        let Some(entry) = entry else {
            return text(refusal.status, &refusal.message);
        };
        let shaped = entry
            .protocol
            .deny(refusal.status, refusal.code, &refusal.message);
        let (mut parts, body) = shaped.into_parts();
        if let Some(seconds) = refusal.retry_after {
            parts
                .headers
                .insert(RETRY_AFTER, HeaderValue::from(seconds));
        }
        Response::from_parts(parts, full(body))
    }

    pub(crate) async fn publish(&self, stream: Stream, event: Event) {
        if let Err(error) = self.services.publisher.publish(stream, event).await {
            tracing::warn!(%error, "an event of the forwarder could not be put in the spool");
        }
    }

    async fn denied(&self, lease: &Lease, model: &str, reason: &str) {
        self.publish(
            Stream::Provider {
                provider_id: lease.provider_id(),
            },
            lease_denied(lease.id(), model, reason),
        )
        .await;
    }

    /// Serves a call of a harness that arrived at the listener under the key of its session.
    pub async fn serve_session(
        self: &Arc<Self>,
        request: Request<Incoming>,
        session: &SessionBinding,
        below_session: &str,
    ) -> Response<Body> {
        self.serve(request, Origin::Session(session), below_session)
            .await
    }

    /// Serves a call that arrived through a tunnel under the lease its ticket names.
    pub async fn serve_ticket(
        self: &Arc<Self>,
        request: Request<Incoming>,
        lease_id: Id,
        thread_id: Option<Id>,
    ) -> Response<Body> {
        let path = request.uri().path().to_owned();
        self.serve(
            request,
            Origin::Ticket {
                lease_id,
                thread_id,
            },
            &path,
        )
        .await
    }

    async fn serve(
        self: &Arc<Self>,
        request: Request<Incoming>,
        origin: Origin<'_>,
        below_session: &str,
    ) -> Response<Body> {
        let lease = match &origin {
            Origin::Session(session) => self
                .leases
                .of_session(session.provider_id, session.thread_id),
            Origin::Ticket {
                lease_id,
                thread_id,
            } => self.leases.of_ticket(*lease_id, *thread_id),
        };
        if let (Origin::Session(session), None) = (&origin, &lease)
            && !self.providers.contains(session.provider_id)
            && let Some(ticket) = session.tickets.llm.as_deref().filter(|t| !t.is_empty())
        {
            return self.through_tunnel(request, below_session, ticket).await;
        }
        self.forward(request, below_session, lease).await
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the steps of a forwarded call are one sequence, read best in one place and in order"
    )]
    async fn forward(
        self: &Arc<Self>,
        request: Request<Incoming>,
        below_session: &str,
        lease: Option<Arc<Lease>>,
    ) -> Response<Body> {
        let (parts, body) = request.into_parts();
        let entry = lease
            .as_ref()
            .and_then(|lease| lease.protocol())
            .and_then(|name| self.protocols.get(name))
            .or_else(|| self.protocols.of_request(&parts.method, below_session));

        // 1. The lease.
        let Some(lease) = lease.filter(|lease| {
            lease.is_open() && lease.mode() == orch_link_contract::commands::LeaseMode::Forwarded
        }) else {
            return self.refuse(
                entry,
                Refusal::new(
                    StatusCode::FORBIDDEN,
                    code::LEASE_CLOSED,
                    "no lease is open for this call: the turn it belongs to has ended, or has not begun",
                ),
            );
        };
        let Some(entry) = entry else {
            return text(StatusCode::NOT_FOUND, "no such address");
        };

        // 2. The method and the path.
        let Some(path) = entry.resolve(&parts.method, below_session) else {
            return self.refuse(
                Some(entry),
                Refusal::new(
                    StatusCode::NOT_FOUND,
                    code::PATH_NOT_ALLOWED,
                    format!(
                        "{} {} is not a request that the {} format lets through",
                        parts.method,
                        below_session,
                        entry.name()
                    ),
                ),
            );
        };

        // 3. The body, and the model it names.
        if parts
            .headers
            .get(CONTENT_ENCODING)
            .is_some_and(|encoding| !encoding.as_bytes().eq_ignore_ascii_case(b"identity"))
        {
            return self.refuse(
                Some(entry),
                Refusal::new(
                    StatusCode::UNSUPPORTED_MEDIA_TYPE,
                    code::UNSUPPORTED_ENCODING,
                    "the request body is encoded; the model it names cannot be read, so it is not forwarded",
                ),
            );
        }
        let limit = usize::try_from(self.max_request_bytes).unwrap_or(usize::MAX);
        let body = match Limited::new(body, limit).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => {
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        code::REQUEST_TOO_LARGE,
                        format!("a request larger than {limit} bytes is not forwarded"),
                    ),
                );
            }
        };
        if let Some(member) = self.protocols.privacy().refused_member(&body) {
            return self.refuse(
                Some(entry),
                Refusal::new(
                    StatusCode::FORBIDDEN,
                    code::REQUEST_REFUSED,
                    format!(
                        "the request holds {member:?}, which Orch does not send to a model's vendor: it is a record of the session, and the program was set not to add it"
                    ),
                ),
            );
        }
        if entry
            .protocol
            .models_request()
            .is_some_and(|(method, listed)| {
                method == parts.method
                    && listed.split_once('?').map_or(listed, |(path, _)| path) == path
            })
        {
            let (parts, bytes) = (entry.models)(lease.allowed_models()).into_parts();
            return Response::from_parts(parts, full(bytes));
        }
        let named = entry.protocol.model_of(&path, &body);
        let model = match named {
            Some(model) if lease.allows(&model) => model,
            other => {
                let shown = other.unwrap_or_default();
                self.denied(&lease, &shown, code::MODEL_NOT_ALLOWED).await;
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::FORBIDDEN,
                        code::MODEL_NOT_ALLOWED,
                        format!(
                            "the model {shown:?} is not one this turn may use: {}",
                            if lease.allowed_models().is_empty() {
                                "it may use none".to_owned()
                            } else {
                                format!("it may use {}", lease.allowed_models().join(", "))
                            }
                        ),
                    ),
                );
            }
        };

        // 4. The allowance.
        if lease.is_spent() {
            self.denied(&lease, &model, code::CAP_REACHED).await;
            return self.refuse(
                Some(entry),
                Refusal::new(
                    StatusCode::FORBIDDEN,
                    code::CAP_REACHED,
                    "this turn has used what it was allowed to",
                ),
            );
        }

        // 5. The provider and the endpoint that speaks the call's protocol.
        let Some(provider) = self.providers.get(lease.provider_id()) else {
            return self.refuse(
                Some(entry),
                Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    code::PROVIDER_UNAVAILABLE,
                    "the provider is not started on this machine",
                )
                .later(),
            );
        };
        let served: Vec<&str> = provider
            .endpoints()
            .iter()
            .map(EndpointEntry::protocol)
            .collect();
        let route = self.protocols.route(entry.name(), &served);
        let (endpoint_protocol, conversion) = match &route {
            Translation::Direct => (entry.name(), None),
            Translation::Translate { to } => (
                to.as_str(),
                self.protocols
                    .translators
                    .iter()
                    .find(|translator| translator.from() == entry.name() && translator.to() == to),
            ),
            Translation::Unserved => (entry.name(), None),
        };
        let Some(endpoint) = provider.endpoint_for(endpoint_protocol) else {
            return self.refuse(
                Some(entry),
                Refusal::new(
                    StatusCode::NOT_FOUND,
                    code::PROTOCOL_NOT_SERVED,
                    format!("no endpoint of this provider speaks {}", entry.name()),
                ),
            );
        };

        // 6. The key, and everything else the endpoint adds.
        let mut headers = parts.headers.clone();
        for name in [
            HOST,
            CONTENT_LENGTH,
            ACCEPT_ENCODING,
            TRANSFER_ENCODING,
            CONNECTION,
        ] {
            headers.remove(name);
        }
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let named: Vec<HeaderName> = headers
            .keys()
            .filter(|name| self.protocols.privacy().strips(name))
            .cloned()
            .collect();
        for name in named {
            headers.remove(name);
        }
        let credential = provider.credential(&self.store);
        let mut method = parts.method.clone();
        let mut sent_path = path.clone();
        let mut sent_body = body;
        let mut sent_query = parts.uri.query().map(str::to_owned);
        let mut converter = None;
        let presenter = translator::presenter(endpoint_protocol, &self.protocols.names());
        if let Some(conversion) = conversion {
            let conversion = Arc::clone(conversion);
            let upstream = translator::UpstreamSide {
                base_url: endpoint.base_url().to_string(),
                options: serde_json::to_value(endpoint.options()).unwrap_or_default(),
                credential: (presenter == translator::Presenter::Translator
                    && provider.presenter().is_none())
                .then(|| credential.clone())
                .flatten(),
            };
            let translated = tokio::task::spawn_blocking(move || {
                conversion
                    .translate_request(&path, sent_body, &upstream)
                    .map(|request| (request, conversion.response_translator()))
            })
            .await;
            let (request, response) = match translated {
                Ok(Ok(result)) => result,
                _ => {
                    return self.refuse(
                        Some(entry),
                        Refusal::new(
                            StatusCode::BAD_GATEWAY,
                            code::PROTOCOL_NOT_SERVED,
                            "the registered translator could not convert this request",
                        ),
                    );
                }
            };
            if request.check_for(presenter).is_err() {
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::BAD_GATEWAY,
                        code::PROTOCOL_NOT_SERVED,
                        "the translator did not follow the endpoint's credential rules",
                    ),
                );
            }
            let Ok(translated_method) = Method::from_bytes(request.method.as_bytes()) else {
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::BAD_GATEWAY,
                        code::PATH_NOT_ALLOWED,
                        "the translator returned an invalid method",
                    ),
                );
            };
            if !request.path.starts_with('/')
                || request.path.starts_with("//")
                || request.path.contains('?')
                || request.path.split('/').any(|part| part == "..")
                || self.protocols.get(endpoint_protocol).is_some_and(|target| {
                    !target.protocol.allowed(&translated_method, &request.path)
                        || target
                            .protocol
                            .model_of(&request.path, &request.body)
                            .as_deref()
                            != Some(model.as_str())
                })
            {
                return self.refuse(Some(entry), Refusal::new(StatusCode::BAD_GATEWAY,
                    code::PATH_NOT_ALLOWED, "the translated request changes the model or leaves the endpoint's allowed paths"));
            }
            method = translated_method;
            sent_path = request.path;
            sent_body = request.body;
            if u64::try_from(sent_body.len()).unwrap_or(u64::MAX) > self.max_request_bytes {
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        code::REQUEST_TOO_LARGE,
                        "the translated request exceeds this runtime's request limit",
                    ),
                );
            }
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            for (name, value) in request.query {
                query.append_pair(&name, &value);
            }
            let query = query.finish();
            sent_query = (!query.is_empty()).then_some(query);
            // Start afresh: headers supplied by a harness, including its credentials, never
            // become headers authorized by a translator.
            headers = HeaderMap::new();
            headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
            for (name, value) in request.headers {
                let (Ok(name), Ok(value)) =
                    (HeaderName::try_from(name), HeaderValue::try_from(value))
                else {
                    return self.refuse(
                        Some(entry),
                        Refusal::new(
                            StatusCode::BAD_GATEWAY,
                            code::PROTOCOL_NOT_SERVED,
                            "the translator returned an invalid header",
                        ),
                    );
                };
                if !is_hop_by_hop(&name) && name != HOST && name != CONTENT_LENGTH {
                    headers.insert(name, value);
                }
            }
            converter = Some(response);
        }
        let endpoint_entry = self.protocols.get(endpoint_protocol);
        let extra_query = match self
            .authorize(
                &provider,
                endpoint,
                endpoint_entry,
                &mut headers,
                credential.as_ref(),
            )
            .await
        {
            Ok(extra) => extra,
            Err(AuthorizationError::Presentation(error)) => {
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::BAD_GATEWAY,
                        code::CREDENTIAL_UNAVAILABLE,
                        error.message,
                    ),
                );
            }
            Err(AuthorizationError::Endpoint(error)) => {
                let message = match error {
                    AuthorizeError::NoCredential | AuthorizeError::SecretMissing(_) => {
                        "the provider has no key for this endpoint".to_owned()
                    }
                    other => other.to_string(),
                };
                return self.refuse(
                    Some(entry),
                    Refusal::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        code::PROVIDER_UNAVAILABLE,
                        message,
                    )
                    .later(),
                );
            }
        };
        let url = upstream_url(endpoint, &sent_path, sent_query.as_deref(), &extra_query);

        // 7. Send it, and hand the answer on as it comes.
        let sent = self
            .upstream
            .send(UpstreamRequest {
                method,
                url,
                headers,
                body: sent_body,
            })
            .await;
        let answer = match sent {
            Ok(answer) => answer,
            Err(error) => {
                let (status, label) = match error {
                    UpstreamError::TimedOut(_) => {
                        (StatusCode::GATEWAY_TIMEOUT, code::UPSTREAM_TIMEOUT)
                    }
                    _ => (StatusCode::BAD_GATEWAY, code::UPSTREAM_UNREACHABLE),
                };
                tracing::warn!(provider = %provider.id(), %error, "a model call could not be forwarded");
                return self.refuse(
                    Some(entry),
                    Refusal::new(status, label, error.to_string()).later(),
                );
            }
        };
        self.note_key(&provider, answer.status).await;
        let watcher = Metered {
            observer: entry.protocol.usage_observer(),
            meter: Meter::start(Arc::clone(&lease)),
            model,
            publisher: Arc::clone(&self.services.publisher),
            runtime: tokio::runtime::Handle::current(),
        };
        let translated = converter.is_some();
        let body = match converter {
            Some(converter) => TranslatedRelay::new(
                Relay::new(answer.body, Box::new(Unwatched), IDLE_TIMEOUT),
                converter,
                Box::new(watcher),
            )
            .boxed(),
            None => Relay::new(answer.body, Box::new(watcher), IDLE_TIMEOUT).boxed(),
        };
        let mut response = Response::new(body);
        *response.status_mut() = answer.status;
        for (name, value) in &answer.headers {
            if !is_hop_by_hop(name)
                && name != CONTENT_LENGTH
                && !(translated && name == CONTENT_ENCODING)
            {
                response.headers_mut().append(name.clone(), value.clone());
            }
        }
        response
    }

    /// Tells the system when the upstream refuses a provider's key, and when it takes it again.
    async fn note_key(&self, provider: &ProviderEntry, status: StatusCode) {
        self.note_key_cause(provider, status, status == StatusCode::UNAUTHORIZED)
            .await;
    }

    /// A whole completion can use its classified body instead of guessing from a forbidden status.
    async fn note_key_cause(&self, provider: &ProviderEntry, status: StatusCode, refused: bool) {
        let report = if refused {
            provider.set_refused(true).then(|| ProviderStatus {
                status: "error".to_owned(),
                detail: Some(format!(
                    "the upstream refused the key with {}",
                    status.as_u16()
                )),
            })
        } else if status.is_success() {
            provider.set_refused(false).then(|| ProviderStatus {
                status: "ready".to_owned(),
                detail: None,
            })
        } else {
            None
        };
        if let Some(report) = report {
            self.publish(
                Stream::Provider {
                    provider_id: provider.id(),
                },
                Event::ProviderStatus(report),
            )
            .await;
        }
    }

    /// Passes a call to the machine that holds the provider's key, through an `llm` tunnel.
    async fn through_tunnel(
        &self,
        request: Request<Incoming>,
        below_session: &str,
        ticket: &str,
    ) -> Response<Body> {
        let entry = self.protocols.of_request(request.method(), below_session);
        let unavailable = |why: String| {
            self.refuse(
                entry,
                Refusal::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    code::PROVIDER_UNAVAILABLE,
                    why,
                )
                .later(),
            )
        };
        let tunnel = match self.services.tunnels.open(ticket).await {
            Ok(tunnel) => tunnel,
            Err(error) => {
                return unavailable(format!(
                    "the machine that holds the key is not reachable: {error}"
                ));
            }
        };
        let (mut sender, connection) = match hyper::client::conn::http1::handshake::<_, Incoming>(
            TokioIo::new(tunnel),
        )
        .await
        {
            Ok(pair) => pair,
            Err(error) => return unavailable(format!("the tunnel could not be used: {error}")),
        };
        tokio::spawn(async move {
            let _ = connection.await;
        });
        let (parts, body) = request.into_parts();
        let mut headers = parts.headers.clone();
        for name in [HOST, CONTENT_LENGTH, TRANSFER_ENCODING, CONNECTION] {
            headers.remove(name);
        }
        // What the harness presented to this machine's listener is for the listener and goes no further.
        headers.remove(hyper::header::AUTHORIZATION);
        headers.remove("x-api-key");
        headers.remove(hyper::header::COOKIE);
        headers.insert(HOST, HeaderValue::from_static(TUNNEL_HOST));
        let mut path_and_query = below_session.to_owned();
        if let Some(query) = parts.uri.query() {
            path_and_query.push('?');
            path_and_query.push_str(query);
        }
        let mut forwarded = match Request::builder()
            .method(parts.method)
            .uri(path_and_query)
            .body(body)
        {
            Ok(forwarded) => forwarded,
            Err(error) => return unavailable(format!("the request cannot be passed on: {error}")),
        };
        *forwarded.headers_mut() = headers;
        let answer = match sender.send_request(forwarded).await {
            Ok(answer) => answer,
            Err(error) => {
                return unavailable(format!(
                    "the machine that holds the key did not answer: {error}"
                ));
            }
        };
        let (mut parts, body) = answer.into_parts();
        let hop: Vec<HeaderName> = parts
            .headers
            .keys()
            .filter(|name| is_hop_by_hop(name) || *name == CONTENT_LENGTH)
            .cloned()
            .collect();
        for name in hop {
            parts.headers.remove(name);
        }
        Response::from_parts(parts, body.map_err(std::io::Error::other).boxed())
    }

    /// Asks one endpoint of a provider for its list of models, presenting the key in that protocol.
    pub async fn ask(
        &self,
        provider: &ProviderEntry,
        endpoint: &EndpointEntry,
        entry: &ProtocolEntry,
    ) -> Result<(StatusCode, Bytes), UpstreamError> {
        let Some((_, listed)) = entry.protocol.models_request() else {
            return Err(UpstreamError::Http(format!(
                "the {} format has no list of models",
                entry.name()
            )));
        };
        self.ask_models_page(provider, endpoint, entry, listed)
            .await
    }

    /// Reads one discovery page only below the original models endpoint, with its original authorization.
    pub async fn ask_models_page(
        &self,
        provider: &ProviderEntry,
        endpoint: &EndpointEntry,
        entry: &ProtocolEntry,
        listed: &str,
    ) -> Result<(StatusCode, Bytes), UpstreamError> {
        let (method, original) = entry
            .protocol
            .models_request()
            .ok_or_else(|| UpstreamError::Http("the protocol has no model list".to_owned()))?;
        let original = original.split_once('?').map_or(original, |(path, _)| path);
        let trusted = upstream_url(endpoint, original, None, &[]);
        let page = match Url::parse(listed) {
            Ok(url) => url,
            Err(_) if listed.starts_with('/') && !listed.starts_with("//") => {
                let (path, query) = listed
                    .split_once('?')
                    .map_or((listed, None), |(path, query)| (path, Some(query)));
                upstream_url(endpoint, path, query, &[])
            }
            Err(_) => {
                return Err(UpstreamError::Address(
                    "a model page must name the original models endpoint".to_owned(),
                ));
            }
        };
        if page.origin() != trusted.origin()
            || page.path() != trusted.path()
            || page.fragment().is_some()
            || !page.username().is_empty()
            || page.password().is_some()
        {
            return Err(UpstreamError::Address(
                "the next model page leaves its original endpoint".to_owned(),
            ));
        }
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let credential = provider.credential(&self.store);
        let extra = self
            .authorize(
                provider,
                endpoint,
                Some(entry),
                &mut headers,
                credential.as_ref(),
            )
            .await
            .map_err(|error| {
                UpstreamError::Http(match error {
                    AuthorizationError::Endpoint(error) => error.to_string(),
                    AuthorizationError::Presentation(error) => error.message,
                })
            })?;
        let mut url = page;
        {
            let mut query = url.query_pairs_mut();
            for (name, value) in extra {
                query.append_pair(&name, &value);
            }
        }
        let (status, _, body) = self
            .upstream
            .fetch(
                UpstreamRequest {
                    method,
                    url,
                    headers,
                    body: Bytes::new(),
                },
                crate::upstream::MAX_WHOLE_REPLY_BYTES,
            )
            .await?;
        self.note_key(provider, status).await;
        Ok((status, body))
    }

    /// One non-streaming text request using the key already held in the provider runtime.
    pub async fn complete(
        &self,
        provider: &ProviderEntry,
        request: &orch_link_contract::commands::ProviderComplete,
    ) -> Result<
        orch_link_contract::commands::ProviderCompleteResult,
        orch_link_contract::commands::CommandError,
    > {
        use orch_link_contract::commands::{
            CommandError, CommandKind, ErrorCode, ProviderCompleteResult,
        };
        let invalid = |message: &str| CommandError::new(ErrorCode::Conflict, message);
        if request.max_output_tokens <= 0 || request.model_name.trim().is_empty() {
            return Err(invalid(
                "a model and a positive output token limit are required",
            ));
        }
        let call = OwnCall {
            provider,
            protocol: &request.protocol,
            model: &request.model_name,
            label: "completion",
            kind: CommandKind::ProviderComplete,
        };
        let (text, usage, timing) = self
            .own_request(
                &call,
                |protocol| completion_request(protocol, request),
                |protocol, body| {
                    protocol
                        .completion_text(body)
                        .map_err(|_| "the upstream did not return a completed text answer")
                },
            )
            .await?;
        Ok(ProviderCompleteResult {
            timing,
            text,
            usage: usage.tokens,
            request_ref: usage.request_ref,
        })
    }

    /// One request that turns texts into vectors, using the key already held in the provider runtime.
    pub async fn embed(
        &self,
        provider: &ProviderEntry,
        request: &orch_link_contract::commands::ProviderEmbed,
    ) -> Result<
        orch_link_contract::commands::ProviderEmbedResult,
        orch_link_contract::commands::CommandError,
    > {
        use orch_link_contract::commands::{
            CommandError, CommandKind, ErrorCode, ProviderEmbedResult, encode_vector,
        };
        if request.inputs.is_empty() || request.model_name.trim().is_empty() {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a model and at least one text are required",
            ));
        }
        let call = OwnCall {
            provider,
            protocol: &request.protocol,
            model: &request.model_name,
            label: "embeddings",
            kind: CommandKind::ProviderEmbed,
        };
        let inputs = request.inputs.len();
        let (vectors, usage, timing) = self
            .own_request(
                &call,
                |protocol| {
                    protocol
                        .embedding_request(&request.model_name, &request.inputs, request.dimensions)
                        .ok_or_else(|| {
                            CommandError::new(
                                ErrorCode::Unsupported,
                                "this protocol cannot make embeddings",
                            )
                        })
                },
                |protocol, body| {
                    protocol
                        .embedding_vectors(body, inputs)
                        .map_err(|_| "the upstream did not return one vector for each text")
                },
            )
            .await?;
        Ok(ProviderEmbedResult {
            timing,
            vectors: vectors.iter().map(|vector| encode_vector(vector)).collect(),
            usage: usage.tokens,
            request_ref: usage.request_ref,
        })
    }

    /// One non-streaming request the system makes on its own behalf with the key held here: the request
    /// `build` writes in the call's protocol goes to the provider's endpoint of that protocol, and `read`
    /// takes what it needs from a successful answer. A request that reached the upstream and failed carries
    /// what it used and how long it took in the error's details, so that the system still counts it.
    async fn own_request<T>(
        &self,
        call: &OwnCall<'_>,
        build: impl FnOnce(
            &dyn WireProtocol,
        ) -> Result<
            (Method, &'static str, Bytes),
            orch_link_contract::commands::CommandError,
        >,
        read: impl FnOnce(&dyn WireProtocol, &Bytes) -> Result<T, &'static str>,
    ) -> Result<
        (T, ObservedUsage, orch_api_contract::usage::RequestTiming),
        orch_link_contract::commands::CommandError,
    > {
        use orch_link_contract::commands::{CommandError, ErrorCode};
        let provider = call.provider;
        let entry = self.protocols.get(call.protocol).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "this runtime does not implement the requested protocol",
            )
        })?;
        let endpoint = provider.endpoint_for(call.protocol).ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "no endpoint of this provider speaks the requested protocol",
            )
        })?;
        let (method, path, body) = build(entry.protocol.as_ref())?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > self.max_request_bytes {
            return Err(CommandError::new(
                ErrorCode::TooLarge,
                format!(
                    "the {} request is larger than this runtime permits",
                    call.label
                ),
            ));
        }
        let credential = provider.credential(&self.store);
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        headers.insert(
            hyper::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        let extra = self
            .authorize(
                provider,
                endpoint,
                Some(entry),
                &mut headers,
                credential.as_ref(),
            )
            .await
            .map_err(|error| match error {
                AuthorizationError::Endpoint(_) => CommandError::new(
                    ErrorCode::LoginRequired,
                    "the provider has no usable credential",
                ),
                AuthorizationError::Presentation(error) => CommandError {
                    code: code::CREDENTIAL_UNAVAILABLE.to_owned(),
                    detail: error.message,
                    retryable: error.retryable,
                    details: None,
                },
            })?;
        let url = upstream_url(endpoint, path, None, &extra);
        let timeout = std::time::Duration::from_secs(u64::from(call.kind.deadline_secs()));
        // This is Orch's own non-streaming model client, not the harness proxy.
        // Whole request time is observable; a buffered reply exposes no first/last token boundary.
        let started = self.services.clock.time();
        let failed = |error: CommandError, code: &str, said: Option<String>| {
            let said = said.map(|mut words| {
                if let Some(credential) = credential
                    .as_ref()
                    .filter(|credential| !credential.expose().is_empty())
                {
                    words = words.replace(credential.expose(), "[redacted]");
                }
                for secret in provider
                    .secrets()
                    .values()
                    .filter(|secret| !secret.is_empty())
                {
                    words = words.replace(secret.expose(), "[redacted]");
                }
                words
            });
            let mut request = orch_agent_contract::requests::UsageRequest {
                id: call.label.to_owned(),
                model: Some(call.model.to_owned()),
                asked: Some(started),
                end: Some(self.services.clock.time()),
                output_streamed: Some(false),
                ..Default::default()
            };
            request.finish(code, Some(&error.detail), said.as_deref());
            let mut details = error
                .details
                .clone()
                .unwrap_or_else(|| serde_json::json!({}));
            details["model_request"] = serde_json::json!(request);
            error.with_details(details)
        };
        let (status, response_headers, body) = tokio::time::timeout(
            timeout,
            self.upstream.fetch(
                UpstreamRequest {
                    method,
                    url,
                    headers,
                    body,
                },
                crate::upstream::MAX_WHOLE_REPLY_BYTES,
            ),
        )
        .await
        .map_err(|_| {
            failed(
                CommandError::new(ErrorCode::Timeout, format!("the {} timed out", call.label)),
                "provider_unreachable",
                None,
            )
        })?
        .map_err(|_| {
            failed(
                CommandError::new(
                    ErrorCode::Internal,
                    format!("the {} upstream could not be reached", call.label),
                ),
                "provider_unreachable",
                None,
            )
        })?;
        let reply: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        if !status.is_success() || reply.get("error").is_some_and(|error| !error.is_null()) {
            let raw = String::from_utf8_lossy(&body);
            let said = reply
                .pointer("/error/message")
                .or_else(|| reply.get("message"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or(raw.as_ref());
            let retry_after_secs = response_headers
                .get(http::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| {
                    value
                        .parse::<i64>()
                        .ok()
                        .filter(|seconds| *seconds >= 0)
                        .or_else(|| {
                            chrono::DateTime::parse_from_rfc2822(value).ok().map(|at| {
                                orch_provider_contract::calls::seconds_until(
                                    orch_types::Time::new(at.with_timezone(&chrono::Utc)),
                                    self.services.clock.time(),
                                )
                            })
                        })
                });
            let classified = orch_provider_contract::failure::classify_body(
                Some(i64::from(status.as_u16())),
                &reply,
                said,
            );
            let code = if classified.code == "provider_unauthorized" {
                self.note_key_cause(provider, status, true).await;
                ErrorCode::LoginRequired
            } else {
                ErrorCode::Internal
            };
            return Err(failed(
                CommandError::new(
                    code,
                    format!("the {} upstream answered {}", call.label, status.as_u16()),
                )
                .with_details(serde_json::json!({"failure": {
                    "code": classified.code, "reason": classified.reason,"retryable":classified.retryable(),
                    "status": status.as_u16(), "retry_after_secs": retry_after_secs,
                    "evidence": classified.evidence,
                }})),
                &classified.code,
                (!said.is_empty()).then(|| said.to_owned()),
            ));
        }
        if let Some((classified, said)) = orch_provider_contract::failure::completed_refusal(&reply)
        {
            self.note_key(provider, status).await;
            let mut error=failed(CommandError::new(ErrorCode::Internal,"The vendor filter refused this request")
                .with_details(serde_json::json!({"failure":{"code":classified.code,"reason":classified.reason,"status":status.as_u16(),"retryable":true}})),&classified.code,Some(said));
            let mut observer = entry.protocol.usage_observer();
            observer.observe(&body);
            if let Some(usage) = observer.usage()
                && let Some(details) = error.details.as_mut()
            {
                details["model_request"]["tokens"] =
                    serde_json::json!(orch_api_contract::usage::RequestTokens::from(usage.tokens));
            }
            return Err(error);
        }
        let answer = read(entry.protocol.as_ref(), &body).map_err(|message| {
            failed(
                CommandError::new(ErrorCode::Internal, message),
                "harness_error",
                None,
            )
        })?;
        let mut observer = entry.protocol.usage_observer();
        observer.observe(&body);
        let usage = observer.usage().ok_or_else(|| {
            failed(
                CommandError::new(
                    ErrorCode::Internal,
                    format!("the {} response did not report usage", call.label),
                ),
                "harness_error",
                None,
            )
        })?;
        self.note_key(provider, status).await;
        let timing = orch_api_contract::performance::request_timing(
            &orch_api_contract::usage::RequestSample {
                asked: Some(started),
                end: Some(self.services.clock.time()),
                output_tokens: Some(usage.tokens.output_tokens),
                reported_tokens: Some(usage.tokens.into()),
                output_streamed: Some(false),
                outcome: orch_api_contract::usage::UsageMeasurement::Told {
                    value: "done".to_owned(),
                },
                ..orch_api_contract::usage::RequestSample::default()
            },
        );
        Ok((answer, usage, timing))
    }
}

/// Builds a completion without allowing model options to replace protocol-owned request fields.
fn completion_request(
    protocol: &dyn WireProtocol,
    request: &orch_link_contract::commands::ProviderComplete,
) -> Result<(Method, &'static str, Bytes), orch_link_contract::commands::CommandError> {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let invalid = |message: &str| CommandError::new(ErrorCode::Conflict, message);
    let (method, path, body) = protocol
        .completion_request(
            &request.model_name,
            &request.system,
            &request.input,
            request.max_output_tokens,
        )
        .ok_or_else(|| {
            CommandError::new(
                ErrorCode::Unsupported,
                "this protocol cannot make text completions",
            )
        })?;
    let body = if let Some(params) = request.params.as_ref() {
        orch_provider_contract::calls::completion_options(params).map_err(invalid)?;
        let mut extra = params
            .as_object()
            .cloned()
            .ok_or_else(|| invalid("completion parameters must be an object"))?;
        if extra.is_empty() {
            body
        } else {
            use orch_model_contract::protocol::{
                ANTHROPIC_MESSAGES, OPENAI_CHAT, OPENAI_RESPONSES,
            };
            if !matches!(
                protocol.name(),
                OPENAI_RESPONSES | OPENAI_CHAT | ANTHROPIC_MESSAGES
            ) {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    "This protocol has not declared completion model options",
                ));
            }
            let has_effort = [
                "/effort",
                "/reasoning_effort",
                "/reasoning/effort",
                "/output_config/effort",
            ]
            .iter()
            .any(|path| params.pointer(path).is_some());
            let effort =
                orch_provider_contract::calls::requested_effort(params).map_err(invalid)?;
            extra.remove("effort");
            extra.remove("reasoning_effort");
            extra.remove("reasoning");
            extra.remove("output_config");
            if has_effort {
                let effort = serde_json::json!(effort);
                match protocol.name() {
                    OPENAI_RESPONSES => {
                        extra.insert("reasoning".to_owned(), serde_json::json!({"effort":effort}));
                    }
                    ANTHROPIC_MESSAGES => {
                        extra.insert(
                            "output_config".to_owned(),
                            serde_json::json!({"effort":effort}),
                        );
                    }
                    _ => {
                        extra.insert("reasoning_effort".to_owned(), effort);
                    }
                }
            }
            let mut payload: serde_json::Value = serde_json::from_slice(&body)
                .map_err(|_| invalid("completion request must be an object"))?;
            let members = payload
                .as_object_mut()
                .ok_or_else(|| invalid("completion request must be an object"))?;
            for (key, value) in extra {
                if members.contains_key(&key) {
                    return Err(invalid(
                        "completion parameters cannot replace the protocol request",
                    ));
                }
                members.insert(key, value);
            }
            bytes::Bytes::from(payload.to_string())
        }
    } else {
        body
    };
    if protocol.model_of(path, &body).as_deref() != Some(request.model_name.as_str()) {
        return Err(invalid("completion parameters changed the selected model"));
    }
    Ok((method, path, body))
}

/// A request the system makes on its own behalf: whose key, in which protocol, for which model, what it is
/// called in what it says, and the command whose deadline bounds it.
struct OwnCall<'a> {
    provider: &'a ProviderEntry,
    protocol: &'a str,
    model: &'a str,
    label: &'static str,
    kind: orch_link_contract::commands::CommandKind,
}

/// A relay whose observation is performed after response conversion instead.
struct Unwatched;

impl Watcher for Unwatched {
    fn piece(&mut self, _: &[u8]) {}
    fn end(&mut self, _: End) {}
}

type ConvertedPiece = (
    Box<dyn translator::ResponseTranslator>,
    Result<Bytes, translator::TranslateError>,
);

/// Converts one response chunk at a time on blocking workers and meters only the resulting format.
/// Pulling the next upstream chunk waits for the preceding conversion and the harness's demand.
struct TranslatedRelay {
    relay: Relay,
    converter: Option<std::sync::Mutex<Box<dyn translator::ResponseTranslator>>>,
    pending: Option<tokio::task::JoinHandle<ConvertedPiece>>,
    watcher: std::sync::Mutex<Option<Box<dyn Watcher>>>,
    ending: bool,
    done: bool,
}

impl TranslatedRelay {
    fn new(
        relay: Relay,
        converter: Box<dyn translator::ResponseTranslator>,
        watcher: Box<dyn Watcher>,
    ) -> Self {
        Self {
            relay,
            converter: Some(std::sync::Mutex::new(converter)),
            pending: None,
            watcher: std::sync::Mutex::new(Some(watcher)),
            ending: false,
            done: false,
        }
    }

    fn finish(&mut self, end: End) {
        self.done = true;
        if let Some(mut watcher) = self
            .watcher
            .get_mut()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            watcher.end(end);
        }
    }

    fn failed(&mut self) -> std::io::Error {
        let message = "the registered translator could not convert the response";
        self.finish(End::Cut(message.to_owned()));
        std::io::Error::other(message)
    }
}

impl Drop for TranslatedRelay {
    fn drop(&mut self) {
        if !self.done {
            self.finish(End::Abandoned);
        }
    }
}

impl hyper::body::Body for TranslatedRelay {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<hyper::body::Frame<Bytes>, Self::Error>>> {
        use std::future::Future;
        use std::pin::Pin;
        use std::task::Poll;
        let this = self.get_mut();
        loop {
            if this.done {
                return Poll::Ready(None);
            }
            if let Some(pending) = this.pending.as_mut() {
                let result = match Pin::new(pending).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(result) => result,
                };
                this.pending = None;
                let (converter, bytes) = match result {
                    Ok((converter, Ok(bytes))) => (converter, bytes),
                    _ => return Poll::Ready(Some(Err(this.failed()))),
                };
                this.converter = Some(std::sync::Mutex::new(converter));
                if !bytes.is_empty() {
                    if let Some(watcher) = this
                        .watcher
                        .get_mut()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .as_mut()
                    {
                        watcher.piece(&bytes);
                    }
                    if this.ending {
                        this.finish(End::Whole);
                    }
                    return Poll::Ready(Some(Ok(hyper::body::Frame::data(bytes))));
                }
                if this.ending {
                    this.finish(End::Whole);
                    return Poll::Ready(None);
                }
            }
            let piece = match Pin::new(&mut this.relay).poll_frame(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Err(error))) => {
                    this.finish(End::Cut("the upstream stream ended early".to_owned()));
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(Some(Ok(frame))) => match frame.into_data() {
                    Ok(bytes) => Some(bytes),
                    Err(_) => continue,
                },
                Poll::Ready(None) => {
                    this.ending = true;
                    None
                }
            };
            let Some(converter) = this.converter.take() else {
                return Poll::Ready(Some(Err(this.failed())));
            };
            let mut converter = converter
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            this.pending = Some(tokio::task::spawn_blocking(move || {
                let bytes = match piece {
                    Some(bytes) => converter.translate_chunk(bytes),
                    None => converter.finish(),
                };
                (converter, bytes)
            }));
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }
}

/// The upstream base with an allowed relative path and the endpoint's configured query.
fn upstream_url(
    endpoint: &EndpointEntry,
    path: &str,
    query: Option<&str>,
    endpoint_query: &[(String, String)],
) -> Url {
    let mut url = endpoint.base_url().clone();
    let joined = format!("{}{path}", url.path().trim_end_matches('/'));
    url.set_path(&joined);
    let mut pieces: Vec<String> = Vec::new();
    pieces.extend(url.query().map(str::to_owned));
    pieces.extend(query.map(str::to_owned));
    if !endpoint_query.is_empty() {
        let mut added = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in endpoint_query {
            added.append_pair(name, value);
        }
        pieces.push(added.finish());
    }
    pieces.retain(|piece| !piece.is_empty());
    let full_query = pieces.join("&");
    url.set_query((!full_query.is_empty()).then_some(full_query.as_str()));
    url
}

/// Watches a forwarded answer: reads what it used as it passes and keeps the lease up to date.
struct Metered {
    observer: Box<dyn UsageObserver>,
    meter: Meter,
    model: String,
    publisher: Arc<dyn Publisher>,
    runtime: tokio::runtime::Handle,
}

impl Watcher for Metered {
    fn piece(&mut self, piece: &[u8]) {
        self.observer.observe(piece);
        if let Some(seen) = self.observer.usage() {
            self.meter.update(&seen.tokens);
        }
    }

    fn end(&mut self, end: End) {
        let seen: Option<ObservedUsage> = self.observer.usage();
        if let Some(seen) = &seen {
            self.meter.update(&seen.tokens);
        }
        if let End::Cut(why) = &end {
            tracing::debug!(%why, "a forwarded answer was cut");
        }
        let Some(event) =
            seen.and_then(|seen| lease_usage(self.meter.lease().id(), &self.model, &seen))
        else {
            return;
        };
        let stream = Stream::Provider {
            provider_id: self.meter.lease().provider_id(),
        };
        let publisher = Arc::clone(&self.publisher);
        self.runtime.spawn(async move {
            if let Err(error) = publisher.publish(stream, event).await {
                tracing::warn!(%error, "a lease.usage could not be put in the spool");
            }
        });
    }
}

/// The endpoint of the listener that serves `/llm/<session key>/…`.
pub struct LlmEndpoint {
    forwarder: Arc<Forwarder>,
}

impl LlmEndpoint {
    /// The endpoint of a forwarder.
    #[must_use]
    pub fn new(forwarder: Arc<Forwarder>) -> Self {
        Self { forwarder }
    }
}

#[async_trait]
impl Endpoint for LlmEndpoint {
    async fn serve(
        &self,
        request: Request<Incoming>,
        session: Arc<SessionBinding>,
        route: Route,
    ) -> Response<Body> {
        let Route::Llm { rest, .. } = route else {
            return text(StatusCode::NOT_FOUND, "no such address");
        };
        self.forwarder.serve_session(request, &session, &rest).await
    }
}

/// Serves the `llm` tunnels other machines open to this one: each carries one HTTP exchange, under the lease
/// its ticket names.
pub struct LlmTunnels {
    forwarder: Arc<Forwarder>,
}

impl LlmTunnels {
    /// The server of a forwarder's tunnels.
    #[must_use]
    pub fn new(forwarder: Arc<Forwarder>) -> Self {
        Self { forwarder }
    }

    /// Registers it for the tunnels of the `llm` kind.
    pub fn register(self, services: &Services) {
        services.tunnels.register(TunnelKind::Llm, Arc::new(self));
    }
}

#[async_trait]
impl TunnelServer for LlmTunnels {
    async fn serve(&self, incoming: IncomingTunnel) {
        let lease_id = incoming
            .service
            .strip_prefix("llm/")
            .and_then(|id| id.parse::<Id>().ok());
        let Some(lease_id) = lease_id else {
            incoming.refuse(OpenErrorCode::NoSuchService, "an llm tunnel names a lease");
            return;
        };
        let thread_id = incoming.claims.thread_id;
        let Ok(tunnel) = incoming.accept() else {
            return;
        };
        let forwarder = Arc::clone(&self.forwarder);
        let service = service_fn(move |request: Request<Incoming>| {
            let forwarder = Arc::clone(&forwarder);
            async move {
                Ok::<_, Infallible>(forwarder.serve_ticket(request, lease_id, thread_id).await)
            }
        });
        let connection = hyper::server::conn::http1::Builder::new()
            .serve_connection(TokioIo::new(tunnel), service);
        if let Err(error) = connection.await {
            tracing::debug!(%error, "an llm tunnel ended with an error");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_model_contract::endpoint::EndpointOptions;

    fn endpoint(base: &str) -> EndpointEntry {
        EndpointEntry::new(
            "openai-responses",
            Url::parse(base).unwrap(),
            EndpointOptions::default(),
        )
    }

    #[test]
    fn the_address_of_a_call_is_the_base_with_the_path_the_query_and_the_endpoints_own_query() {
        let plain = endpoint("https://api.example.com/v1");
        assert_eq!(
            upstream_url(&plain, "/responses", None, &[]).as_str(),
            "https://api.example.com/v1/responses"
        );
        let slash = endpoint("https://api.example.com/v1/");
        assert_eq!(
            upstream_url(&slash, "/responses", Some("beta=true"), &[]).as_str(),
            "https://api.example.com/v1/responses?beta=true"
        );
        let both = upstream_url(
            &plain,
            "/models",
            Some("limit=5"),
            &[("api-version".to_owned(), "2025 01".to_owned())],
        );
        assert_eq!(both.query(), Some("limit=5&api-version=2025+01"));
        let bare = endpoint("http://127.0.0.1:9000");
        assert_eq!(
            upstream_url(&bare, "/v1/messages", None, &[]).as_str(),
            "http://127.0.0.1:9000/v1/messages"
        );
        let versioned = endpoint("https://gateway.example/openai?version=3");
        assert_eq!(
            upstream_url(&versioned, "/responses", Some("beta=true"), &[]).as_str(),
            "https://gateway.example/openai/responses?version=3&beta=true",
            "a query the base address has is kept"
        );
    }

    #[test]
    fn headers_that_name_the_client_are_dropped_by_name_or_prefix_and_others_stay() {
        let privacy = Privacy::new(
            &["X-Client-*".to_owned(), "x-session-token".to_owned()],
            &[],
        );
        let name = |text: &str| HeaderName::from_bytes(text.as_bytes()).unwrap();
        assert!(privacy.strips(&name("x-client-user-id")));
        assert!(privacy.strips(&name("x-client-compact")));
        assert!(privacy.strips(&name("x-session-token")));
        assert!(!privacy.strips(&name("x-session-token-2")));
        assert!(!privacy.strips(&name("user-agent")));
        assert!(!Privacy::default().strips(&name("x-client-user-id")));
    }

    #[test]
    fn a_body_with_a_refused_member_at_its_top_level_is_named_and_one_that_only_mentions_it_is_not()
    {
        let privacy = Privacy::new(&[], &["upload_log".to_owned(), "upload_plugins".to_owned()]);
        let body = br#"{"model":"m","messages":[{"role":"user","content":"upload_log"}],"upload_plugins":{"version":1}}"#;
        assert_eq!(
            privacy.refused_member(body).as_deref(),
            Some("upload_plugins")
        );
        let nested = br#"{"model":"m","extra":{"upload_log":1},"messages":[]}"#;
        assert_eq!(
            privacy.refused_member(nested),
            None,
            "only the top level of a request is the vendor's own"
        );
        assert_eq!(privacy.refused_member(b"not json"), None);
        assert_eq!(Privacy::default().refused_member(body), None);
    }
}
