//! HTTP registration with permission and token metadata.
//!
//! A request is served only if the contract declares it. [`Registry::on`]
//! takes a declared endpoint and a handler whose arguments and result are
//! that endpoint's own query, body and reply types, so a handler cannot
//! read or answer anything else than what clients were generated from.
//! Around the handler, the same for every request: an identifier, who is
//! calling and whether that is enough for the endpoint, the query and the
//! body read strictly, and failures answered as problems.

use crate::middleware::authentication::{self, Authed, Caller};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::state::App;
use axum::body::Body;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{FromRequestParts, RawPathParams, Request, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, on};
use chrono::{DateTime, Utc};
use http::{HeaderMap, HeaderValue, StatusCode, header};
use orch_api_contract::common::{
    ErrorCode, FieldFault, MAX_BODY_BYTES, MAX_JSON_BYTES, Problem, REQUEST_ID_HEADER,
};
use orch_api_contract::routes::{Access, Endpoint, Method, Route, Shape};
use orch_types::Id;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::net::IpAddr;
use std::sync::Arc;

/// What a handler is given about its request, beside the query and body.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// One logical browser opening, optionally shared with its live subscription.
    pub(crate) opening: Id,
    /// Whether this whitelisted reader explicitly requested deleted content.
    pub inspect_deleted: bool,
    /// The exact retained thing authorized before its stored payload is built.
    pub(crate) inspection: Option<crate::middleware::inspection::Info>,
    /// The shared state.
    pub app: App,
    /// Who is calling.
    pub caller: Caller,
    /// The request's identifier, as in its answer.
    pub request_id: String,
    /// The moment the handler was prepared, after JSON decoding; action checks
    /// sample the application's clock again inside their transaction.
    pub now: DateTime<Utc>,
    /// The request's headers.
    pub headers: HeaderMap,
    /// The caller's address, from its connection and explicitly trusted proxies.
    pub remote: Option<IpAddr>,
    /// What the endpoint declares.
    pub route: Route,
    params: Vec<(String, String)>,
}

impl Ctx {
    /// A context for a tool's projections; authority for its writes is checked separately in its transaction.
    pub(crate) fn for_tool(app: App, caller: Caller, route: Route, id: Id) -> Self {
        let now = app.clock.now();
        Self {
            opening: orch_types::new_id(),
            inspect_deleted: false,
            inspection: None,
            app,
            caller,
            request_id: format!("tool:{id}"),
            now,
            headers: HeaderMap::new(),
            remote: None,
            route,
            params: vec![("id".to_owned(), id.to_string())],
        }
    }

    /// Retains the reader's identity while checking one requested child separately.
    pub(crate) fn for_id(&self, id: Id) -> Self {
        let mut child = self.clone();
        for (name, value) in &mut child.params {
            if name == "id" {
                *value = id.to_string();
            }
        }
        child.inspection = None;
        child
    }
    /// The signed-in caller; refused with `auth.required` when there is none.
    pub fn user(&self) -> ApiResult<&Arc<Authed>> {
        match &self.caller {
            Caller::User(authed) => Ok(authed),
            Caller::Anonymous => Err(ApiError::new(ErrorCode::AuthRequired, "sign in first")),
        }
    }

    /// The path parameter `name` as it was written, where the path has it (nothing is logged).
    pub(crate) fn param_if_any(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
    }

    /// The path parameter `name` as it was written.
    pub fn param(&self, name: &str) -> ApiResult<&str> {
        self.params
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value.as_str()))
            .ok_or_else(|| ApiError::internal(format!("the path has no parameter {name}")))
    }

    /// The path parameter `name` as an identifier. Something that is not
    /// an identifier names nothing: `not_found`.
    pub fn id(&self, name: &str) -> ApiResult<Id> {
        self.param(name)?.parse().map_err(|_| ApiError::not_found())
    }

    /// The value of the cookie `name`.
    #[must_use]
    pub fn cookie(&self, name: &str) -> Option<&str> {
        authentication::cookie(&self.headers, name)
    }
}

// Authority mode is chosen before the handler opens any transaction. Even when
// a body or stored kind makes the change optional, that request must never start
// with a shared epoch and upgrade it after taking object or identity locks.
/// Whether this endpoint can change authority, including a rolled-back trial.
pub(crate) fn changes_authority(route: &Route) -> bool {
    matches!(
        route.name,
        // Pages, roles and previews that write and roll back a trial page.
        "NodeRoleCreate" | "RoleUpdate" | "RoleDelete" |
        "PermissionPageUpdate" | "PermissionPagePreview" | "SharePreviewCreate" |
        "NodeDefaultsUpdate" | "NodeDefaultsPreview" |
        "AdminDefaultsUpdate" | "AdminDefaultsPreview" |
        "NodeTransfer" | "ObjectTransfer" |
        // Memberships, groups, suspensions, service identities and invitations.
        "OrgMemberRemove" | "OrgMemberMove" | "OrgMemberSuspend" | "OrgMemberResume" |
        "OrgOutsiderRemove" |
        "OrgGroupCreate" | "GroupUpdate" | "GroupDelete" | "GroupMemberAdd" | "GroupMemberRemove" |
        "GroupHolderAdd" | "GroupHolderRemove" |
        "OrgServiceAccountCreate" | "ServiceAccountDelete" |
        "ServiceAccountTokenCreate" | "ServiceAccountTokenDelete" | "InvitationAccept" |
        // Another person's standing, or a token's admission to a shared org.
        "OrgTokenRefuse" |
        "AdminUserCreate" | "AdminUserUpdate" | "AdminUserResetLinkCreate" | "AdminOrgUpdate" |
        "AdminUserOrgInvitationGive" | "AdminUserOrgInvitationTakeBack" |
        // Changes to where an existing page gets its authority.
        "OrgMove" | "ProjectMove" | "TopicMove" | "NodeDelete" | "NodeRestore" |
        "ThreadMove" | "ThreadMovePreview" |
        "NodeArchive" | "NodeUnarchive" | "NodeArchivePreview" | "NodeUnarchivePreview" |
        "ThreadBindingSet" | "ThreadEnvironmentSet" | "ThreadForkCreate" | "ForkBatchCreate" |
        "DocumentMove" | "DocumentDelete" | "DocumentRestore" |
        "ChatMemberAdd" | "ChatMemberRemove" | "ChatAgentPostingSet" | "ChatPersonGrant" |
        // Shared resources, their home, availability and permission policies.
        "ProviderUpdate" | "ProviderDelete" |
        "ProviderDisable" | "ProviderEnable" |
        // Credentials, their reported expiry and hosted availability can change what work may run.
        "ProviderKeySet" | "ProviderSecretSet" | "ProviderFactsOverridesSet" | "ProviderSourcesSet" |
        "ProviderResetConsume" |
        "ProviderRefresh" | "ProviderHarnessRestart" |
        "ProviderLogout" |
        "ProviderStart" | "ProviderStop" | "ProviderLoginStart" | "ProviderLoginInput" | "ProviderLoginCancel" |
        "ProviderHandoverReturn" |
        "HostRuntimeStop" | "HostRuntimeDelete" |
        "AgentUpdate" | "AgentDelete" |
        "TemplateUpdate" | "TemplateDelete" |
        "McpServerUpdate" | "McpServerDelete" | "McpServerReimport" |
        "SkillUpdate" | "SkillDelete" |
        "HostEnrollmentTokenCreate" | "HostUpdate" | "HostDelete" | "HostDeleteCreate" | "LossDeclare" | "EnvironmentRecoverCreate" | "McpOfferingUse" |
        "MachineRevoke" | "MachinePolicyUpdate" | "MachineProbe" |
        "EnvironmentUpdate" |
        "EnvironmentDisable" | "EnvironmentEnable" | "EnvironmentDelete" | "EnvironmentCapabilityRevoke" |
        "EnvironmentRestore" | "EnvironmentPurge" | "EnvironmentKeep" |
        "NodeSettingSet" | "NodeSettingDelete" | "AdminSettingSet" | "AdminSettingDelete" |
        // Sign-in paths, approval defaults, executable builds and usable grants.
        "AdminIdentityProviderCreate" | "AdminIdentityProviderUpdate" | "AdminIdentityProviderDelete" |
        "AdminCapabilitySet" | "AdminCapabilityDelete" | "AdminHarnessSet" |
        "InvitationCreate" | "InvitationDelete" |
        // Stored approval/share kinds can grant entries or chat membership.
        "ApprovalDecide" | "ShareAgentCreate" | "ShareAgentUpdate" | "ShareAgentDelete" |
        "EnvironmentPortProgramUpdate" | "EnvironmentPortOpeningCreate" |
        "PortOpeningUpdate" | "PortOpeningDelete" | "EnvironmentPortCapabilityGrant"
    )
}

/// Whether an authority change made by this endpoint is about the root org of the place its path
/// names alone: it moves that org's authorization epoch at its commit, where the writes
/// that decided under it are checked. A change that can reach another root org, or every one
/// (moves, transfers, deletions and restores, forks, an accepted invitation), and one whose path
/// names no node or document, moves the system's epoch, which every write checks.
pub(crate) fn changes_one_place(route: &Route) -> bool {
    !matches!(
        route.name,
        "OrgMove"
            | "ProjectMove"
            | "TopicMove"
            | "ThreadMove"
            | "ThreadMovePreview"
            | "DocumentMove"
            | "NodeTransfer"
            | "ObjectTransfer"
            | "NodeDelete"
            | "NodeRestore"
            | "DocumentDelete"
            | "DocumentRestore"
            | "ThreadForkCreate"
            | "ForkBatchCreate"
            | "InvitationAccept"
    )
}

/// Whether this endpoint's change of authority only adds authority: an invitation made or taken
/// back before use, or accepted (a membership given), an enrollment token, a group or a service
/// identity or token made, a member or holder added. No decision made before it can be wrong after
/// it, so it moves no authorization epoch and refuses nobody's write; the cache still moves.
pub(crate) fn only_grants(route: &Route) -> bool {
    matches!(
        route.name,
        "InvitationCreate"
            | "InvitationDelete"
            | "InvitationAccept"
            | "HostEnrollmentTokenCreate"
            | "OrgGroupCreate"
            | "GroupMemberAdd"
            | "GroupHolderAdd"
            | "OrgServiceAccountCreate"
            | "ServiceAccountTokenCreate"
            | "ChatMemberAdd"
            | "AdminUserOrgInvitationGive"
    )
}

/// Whether this endpoint changes a person's standing in the whole system (a system admin given or
/// taken, a person suspended or made): it moves the epoch of the system's administrators alone
/// (`transaction_guard::STANDING`), which orders two such changes, while the person's own writes
/// are ordered by their user row, which the change locks.
pub(crate) fn changes_standing(route: &Route) -> bool {
    matches!(
        route.name,
        "AdminUserCreate" | "AdminUserUpdate" | "AdminUserResetLinkCreate"
    )
}

// Credential changes conflict with their own person's actions. A fresh child
// cannot revoke authority on an existing object, and pins its parent instead.
fn lock_scope(ctx: &Ctx) -> orch_db::queries::transaction_guard::Scope {
    use orch_db::queries::transaction_guard::Scope;
    if changes_authority(&ctx.route) {
        Scope::Authority
    } else if matches!(
        ctx.route.name,
        "SetupCreate"
            | "AuthLogin"
            | "AuthOidcCallback"
            | "AuthRefresh"
            | "AuthLogout"
            | "AuthRecoveryConfirm"
            | "AuthEmailVerify"
            | "MeUpdate"
            | "MeIdentityDelete"
            | "MePasswordChange"
            | "MeTotpSetup"
            | "MeTotpEnable"
            | "MeTotpDisable"
            | "MeRecoveryCodesRegenerate"
            | "MeEmailCodeEnable"
            | "MeEmailCodeDisable"
            | "MeEmailCodeSend"
            | "MeSecondStepVerify"
            | "MeIdentityLink"
            | "MeAdminModeSet"
            | "MeSessionDelete"
            | "MeSessionOthersDelete"
            | "MeTokenCreate"
            | "MeTokenDelete"
    ) {
        Scope::Personal {
            user: ctx.user().ok().map(|actor| actor.user.id),
        }
    } else if matches!(
        ctx.route.name,
        "AuthRegister"
            | "OrgCreate"
            | "OrgChildCreate"
            | "ProjectCreate"
            | "ProjectChildCreate"
            | "TopicCreate"
            | "ThreadCreate"
            | "ThreadAdopt"
            | "ThreadSideCreate"
            | "NodeDocumentCreate"
            | "NodeDocumentCopy"
            | "NodeDocumentImport"
            | "OrgChatCreate"
            | "ProviderCreate"
            | "MachineProviderCreate"
            | "OrgAgentCreate"
            | "AdminAgentCreate"
            | "MachineAgentCreate"
            | "TemplateCreate"
            | "TemplateImport"
            | "MountCreate"
            | "McpServerCreate"
            | "SkillCreate"
            | "EnvironmentCreate"
            | "MachineEnvironmentCreate"
            | "EnvironmentMcpImportCreate"
            | "EnvironmentMcpServerCreate"
    ) {
        Scope::Creation
    } else {
        Scope::Action
    }
}

fn may_end_admission(route: &Route) -> bool {
    (changes_authority(route)
        || matches!(
            route.name,
            "AuthLogout"
                | "AuthRecoveryConfirm"
                | "MePasswordChange"
                | "MeIdentityDelete"
                | "MeTotpDisable"
                | "MeEmailCodeDisable"
                | "MeSessionDelete"
                | "MeSessionOthersDelete"
                | "MeTokenDelete"
        ))
        && crate::ports_runtime::may_end_admission(route.name)
}

/// A successful answer: the declared reply, and where needed another
/// status than the declared one, or headers (a cookie, a location).
#[derive(Debug)]
pub struct Answer<T> {
    /// The reply.
    pub body: T,
    /// A status other than the endpoint's declared one.
    pub status: Option<StatusCode>,
    /// Headers to add.
    pub headers: HeaderMap,
}

impl<T> From<T> for Answer<T> {
    fn from(body: T) -> Self {
        Self {
            body,
            status: None,
            headers: HeaderMap::new(),
        }
    }
}

impl<T> Answer<T> {
    /// Answers with `status` in place of the declared one.
    #[must_use]
    pub fn status(mut self, status: StatusCode) -> Self {
        self.status = Some(status);
        self
    }

    /// Adds a header; a value that cannot be a header is left out.
    #[must_use]
    pub fn header(mut self, name: header::HeaderName, value: &str) -> Self {
        if let Ok(value) = HeaderValue::from_str(value) {
            self.headers.append(name, value);
        }
        self
    }
}

/// The requests the application serves.
#[derive(Debug, Default)]
pub struct Registry {
    router: axum::Router<App>,
    bound: Vec<Route>,
    /// Whether handlers registered from now on may answer a changing request without checking their action
    /// guard although the ledger says they do; only tests that measure or capture such a request set it.
    unguarded: bool,
}

fn filter(method: Method) -> MethodFilter {
    match method.as_str() {
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "PATCH" => MethodFilter::PATCH,
        "DELETE" => MethodFilter::DELETE,
        _ => MethodFilter::GET,
    }
}

fn remote(app: &App, parts: &http::request::Parts) -> Option<IpAddr> {
    let peer = parts
        .extensions
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()?;
    Some(orch_config::client_address::resolve_observed(
        peer.0.ip(),
        &parts.headers,
        &app.config.api.trusted_proxy_cidrs,
        app.clock.now().timestamp(),
    ))
}

/// A request's future, which every runtime thread may serve: what it holds across a wait is `Send`.
fn sent<T: Future + Send>(future: T) -> T {
    future
}

fn faults_of(path: &str, message: &str) -> ApiError {
    let path = if path == "." { "" } else { path };
    Problem::invalid(vec![FieldFault {
        path: path.to_owned(),
        message: message.to_owned(),
    }])
    .into()
}

fn read_query<Q: DeserializeOwned>(route: &Route, query: Option<&str>) -> ApiResult<Q> {
    if matches!(route.query, Shape::None) {
        // A request without declared parameters takes none.
        return match query {
            None | Some("") => {
                serde_json::from_value(serde_json::Value::Null).map_err(ApiError::internal)
            }
            Some(_) => Err(faults_of("", "this request takes no parameters")),
        };
    }
    // A parameter given several times is a list, for a member that is one.
    let pairs = form_urlencoded::parse(query.unwrap_or("").as_bytes());
    serde_path_to_error::deserialize(serde_html_form::Deserializer::new(pairs))
        .map_err(|error| faults_of(&error.path().to_string(), &error.inner().to_string()))
}

/// The most of a refused body that is still read, and dropped, before the refusal is answered: as
/// much as the shipped edge lets through in one request. The edge has the whole body before it
/// sends any of it on; an answer given while it is still sending is lost with the connection that
/// closes under it, and the caller is told 502 by the edge instead of the 413 given here.
const REFUSED_BODY_DRAIN_BYTES: usize = MAX_BODY_BYTES;
/// How long reading and dropping the rest of a refused body may take. Behind the edge the rest is
/// already buffered and arrives at once; a caller that sends slowly to the API itself is not waited
/// for longer than this, and its connection is closed after the answer.
const REFUSED_BODY_DRAIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Why a JSON body was not taken.
enum Unread {
    /// It is longer than the limit; the rest was read and dropped within the drain's bounds.
    TooLarge,
    /// The connection failed while it was read.
    Failed,
}

/// Reads a body of at most `limit` bytes. A longer one is refused only after what is left of it,
/// up to [`REFUSED_BODY_DRAIN_BYTES`], has been read and dropped, so that whoever sends it on (the
/// edge) still has a connection to read the refusal from.
async fn read_bounded(mut body: Body, limit: usize) -> Result<bytes::Bytes, Unread> {
    use http_body_util::BodyExt;
    let mut kept = bytes::BytesMut::new();
    while let Some(frame) = body.frame().await {
        let Ok(data) = frame.map_err(|_| Unread::Failed)?.into_data() else {
            continue;
        };
        if kept.len().saturating_add(data.len()) > limit {
            let mut read = kept.len().saturating_add(data.len());
            drop(kept);
            let _ = tokio::time::timeout(REFUSED_BODY_DRAIN, async {
                while read < REFUSED_BODY_DRAIN_BYTES {
                    match body.frame().await {
                        Some(Ok(frame)) => {
                            if let Ok(data) = frame.into_data() {
                                read = read.saturating_add(data.len());
                            }
                        }
                        Some(Err(_)) | None => break,
                    }
                }
            })
            .await;
            return Err(Unread::TooLarge);
        }
        kept.extend_from_slice(&data);
    }
    Ok(kept.freeze())
}

/// The bytes of a JSON body, kept so that a request refused at its commit can be served once more.
async fn read_body_bytes(route: &Route, body: Body) -> ApiResult<bytes::Bytes> {
    if matches!(route.body, Shape::None) {
        return Ok(bytes::Bytes::new());
    }
    read_bounded(body, MAX_JSON_BYTES)
        .await
        .map_err(|unread| match unread {
            Unread::TooLarge => ApiError::new(ErrorCode::RequestTooLarge, "the body is too large")
                .with("limit", MAX_JSON_BYTES),
            Unread::Failed => {
                ApiError::new(ErrorCode::RequestInvalid, "the body could not be read")
            }
        })
}

fn decode_body<B: DeserializeOwned>(route: &Route, bytes: &[u8]) -> ApiResult<B> {
    if matches!(route.body, Shape::None) {
        return serde_json::from_value(serde_json::Value::Null).map_err(ApiError::internal);
    }
    let bytes: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) {
        b"{}"
    } else {
        bytes
    };
    let mut json = serde_json::Deserializer::from_slice(bytes);
    serde_path_to_error::deserialize(&mut json)
        .map_err(|error| faults_of(&error.path().to_string(), &error.inner().to_string()))
}

/// Whether a request whose commit was refused because who may do what changed meanwhile is
/// served once more, deciding again under the new authority: an ordinary write or a change that only
/// grants, which has done nothing outside its transaction that a second serving would repeat. Any other
/// change of authority, a personal credential and a raw body are answered as they failed
/// (`conflict.stale_revision`).
fn retried_after_refusal(route: &Route) -> bool {
    route.method.as_str() != "GET"
        && (!changes_authority(route) || only_grants(route))
        && matches!(
            route.access,
            Access::User | Access::Session | Access::Permission(_) | Access::Rule(_)
        )
}

async fn open(
    app: &App,
    route: Route,
    parts: &mut http::request::Parts,
    request_id: &str,
) -> ApiResult<Ctx> {
    let inspect_deleted = crate::middleware::inspection::requested(&route, parts.uri.query())?;
    let now = app.clock.now();
    let caller = match route.access {
        Access::Public => Caller::Anonymous,
        Access::Cookie => {
            if !crate::sign_in::has_csrf(&parts.headers) {
                return Err(ApiError::new(
                    ErrorCode::AuthCsrf,
                    "the request lacks its required CSRF header value",
                ));
            }
            Caller::Anonymous
        }
        Access::User | Access::Session | Access::Permission(_) | Access::Rule(_) => {
            let caller = authentication::authenticate(app, &parts.headers, now).await?;
            let Caller::User(authed) = &caller else {
                return Err(ApiError::new(ErrorCode::AuthRequired, "sign in first"));
            };
            if matches!(route.access, Access::Session) && !authed.is_session() {
                return Err(ApiError::new(
                    ErrorCode::AuthSessionRequired,
                    "this is done in a browser session, not with a token",
                ));
            }
            caller
        }
    };
    let params = RawPathParams::from_request_parts(parts, app)
        .await
        .map(|params| {
            params
                .iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect()
        })
        .unwrap_or_default();
    // The complete handle is now available to the live browser transport.
    crate::realtime::browsers::install(app);
    crate::realtime::displays::install(app);
    let mut ctx = Ctx {
        opening: orch_types::new_id(),
        inspect_deleted,
        inspection: None,
        app: app.clone(),
        caller,
        request_id: request_id.to_owned(),
        now,
        headers: parts.headers.clone(),
        remote: remote(app, parts),
        route,
        params,
    };
    ctx.opening = crate::middleware::admin_openings::nonce(&ctx)?;
    if inspect_deleted {
        let mut conn = app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        ctx.inspection = crate::middleware::inspection::prepare_query(
            &ctx,
            &mut conn,
            &principal,
            parts.uri.query(),
        )
        .await?;
    }
    if let Access::Permission(permission) = route.access {
        authorization::require_on_path(&ctx, permission).await?;
        // A granted use of a thing in an org is when its caller was last seen there.
        authorization::note_use(&ctx).await;
    }
    Ok(ctx)
}

fn finish(mut response: Response, request_id: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

impl Registry {
    /// No requests yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lets the handlers registered after this answer a changing request without checking their action
    /// guard, which is otherwise refused in a build with debug assertions for a request the ledger names
    /// `guarded`. Only for a test that measures a write without its guard or captures a request's context;
    /// the application never calls it.
    pub fn allow_unguarded(&mut self) -> &mut Self {
        self.unguarded = true;
        self
    }

    /// Serves `endpoint` with `handler`, whose query, body and reply are
    /// the endpoint's own.
    pub fn on<E, F, Fut, R>(&mut self, endpoint: E, handler: F) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        let _ = endpoint;
        let route = E::ROUTE;
        let unguarded = self.unguarded;
        let serve = move |State(app): State<App>, request: Request| {
            let handler = handler.clone();
            async move {
                let request_id = orch_types::new_id().to_string();
                let (mut parts, body) = request.into_parts();
                let outcome: ApiResult<Response> = sent(async {
                    // Public requests still share this future's permission/inspection branches.
                    // Keep that common authorization state off the request's inline stack.
                    let mut ctx = Box::pin(open(&app, route, &mut parts, &request_id)).await?;
                    let query: E::Query = read_query(&route, parts.uri.query())?;
                    let body_bytes = read_body_bytes(&route, body).await?;
                    let body: E::Body = decode_body(&route, &body_bytes)?;
                    ctx.now = app.clock.now();
                    let reply_ctx = ctx.clone();
                    let read_ids = ctx
                        .inspection
                        .as_ref()
                        .map(|info| info.reads.clone())
                        .unwrap_or_default();
                    let audit_origin = crate::sign_in::origin(&ctx);
                    let opening = ctx.opening;
                    let opening_session = ctx.user().ok().and_then(|user| match user.via {
                        authentication::Via::Session { session_id } => Some(session_id),
                        _ => None,
                    });
                    // The development guard names this request in what it reports.
                    macro_rules! serve_once {
                        ($ctx:expr, $query:expr, $body:expr) => {{
                            let ctx: Ctx = $ctx;
                            let scope = lock_scope(&ctx);
                            let work = Box::pin(handler(ctx, $query, $body));
                            orch_db::pool::serving_watched(
                                route.name,
                                transaction_guard::watch(orch_db::audit::scope_origin(
                                    audit_origin.clone(),
                                    orch_db::queries::transaction_guard::request_scope(
                                        scope,
                                        orch_db::queries::trace::read_scope(
                                            &read_ids,
                                            orch_db::audit::scope_browser_opening(
                                                opening_session,
                                                opening,
                                                work,
                                            ),
                                        ),
                                    ),
                                )),
                            )
                            .await
                        }};
                    }
                    let retry_ctx = retried_after_refusal(&route).then(|| ctx.clone());
                    let ((answer, guarded), refused) = serve_once!(ctx, query, body);
                    // The handler's own result is turned into the answer at once: only that is kept across a wait.
                    let answer: ApiResult<Answer<E::Reply>> = answer.map(Into::into);
                    let (answer, guarded) = if refused
                        && answer.is_err()
                        && let Some(mut ctx) = retry_ctx
                    {
                        // Refused at its commit because who may do what changed meanwhile: decided
                        // again, once, under the authority that now holds.
                        ctx.now = app.clock.now();
                        let query: E::Query = read_query(&route, parts.uri.query())?;
                        let body: E::Body = decode_body(&route, &body_bytes)?;
                        let ((again, guarded), _) = serve_once!(ctx, query, body);
                        (again.map(Into::into), guarded)
                    } else {
                        (answer, guarded)
                    };
                    let answer: Answer<E::Reply> = answer?;
                    if may_end_admission(&route) {
                        // What it changed can end a saved port admission: that is closed, with
                        // its live channels, before this answer leaves.
                        // Even sign-in requests that skip this branch carry its future type.
                        Box::pin(crate::ports_runtime::close_ended(&app)).await;
                    }
                    let status = answer
                        .status
                        .or_else(|| StatusCode::from_u16(route.status).ok())
                        .unwrap_or(StatusCode::OK);
                    if status.is_success() {
                        if !unguarded {
                            transaction_guard::require_seen(&route, guarded)?;
                        }
                        crate::middleware::inspection::finish(&reply_ctx).await?;
                    }
                    let mut response = if matches!(route.reply, Shape::None) {
                        status.into_response()
                    } else {
                        let mut value =
                            serde_json::to_value(&answer.body).map_err(ApiError::internal)?;
                        if status.is_success() {
                            Box::pin(crate::middleware::admin_openings::finish_json(
                                &reply_ctx, &value,
                            ))
                            .await?;
                            // The handler has completed and released its transaction. Bookkeeping
                            // reuses the authorized projection and cannot turn a good page into an error.
                            let _ = Box::pin(crate::routes::personal::viewed::record(
                                &reply_ctx, &value,
                            ))
                            .await;
                        }
                        crate::middleware::inspection::banner(
                            &mut value,
                            reply_ctx.inspection.as_ref(),
                        );
                        let bytes = serde_json::to_vec(&value).map_err(ApiError::internal)?;
                        let mut response = (status, bytes).into_response();
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("application/json"),
                        );
                        response
                    };
                    response.headers_mut().extend(answer.headers);
                    crate::middleware::inspection::headers(&mut response, &reply_ctx)?;
                    Ok(response)
                })
                .await;
                match outcome {
                    Ok(response) => finish(response, &request_id),
                    Err(error) => error.into_response_for(&request_id),
                }
            }
        };
        self.add(route, on(filter(route.method), serve))
    }

    /// Serves `endpoint`, whose body or reply is not JSON, with a handler
    /// that reads the body and writes the answer itself.
    pub fn on_raw<E, F, Fut>(&mut self, endpoint: E, handler: F) -> &mut Self
    where
        E: Endpoint,
        F: Fn(Ctx, E::Query, Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<Response>> + Send + 'static,
    {
        let _ = endpoint;
        let route = E::ROUTE;
        let unguarded = self.unguarded;
        let serve = move |State(app): State<App>, request: Request| {
            let handler = handler.clone();
            async move {
                let request_id = orch_types::new_id().to_string();
                let (mut parts, body) = request.into_parts();
                let outcome: ApiResult<Response> = async {
                    let ctx = Box::pin(open(&app, route, &mut parts, &request_id)).await?;
                    let query: E::Query = read_query(&route, parts.uri.query())?;
                    let reply_ctx = ctx.clone();
                    let read_ids = ctx
                        .inspection
                        .as_ref()
                        .map(|info| info.reads.clone())
                        .unwrap_or_default();
                    let audit_origin = crate::sign_in::origin(&ctx);
                    let (response, guarded) = orch_db::pool::serving(
                        route.name,
                        transaction_guard::watch(orch_db::audit::scope_origin(
                            audit_origin,
                            orch_db::queries::transaction_guard::request_scope(
                                lock_scope(&ctx),
                                orch_db::queries::trace::read_scope(
                                    &read_ids,
                                    orch_db::audit::scope_browser_opening(
                                        reply_ctx.user().ok().and_then(|user| match user.via {
                                            authentication::Via::Session { session_id } => {
                                                Some(session_id)
                                            }
                                            _ => None,
                                        }),
                                        reply_ctx.opening,
                                        Box::pin(handler(ctx, query, body)),
                                    ),
                                ),
                            ),
                        )),
                    )
                    .await;
                    let mut response = response?;
                    if response.status().is_success() {
                        Box::pin(crate::middleware::admin_openings::finish_raw(&reply_ctx)).await?;
                        if !unguarded {
                            transaction_guard::require_seen(&route, guarded)?;
                        }
                        if may_end_admission(&route) {
                            // As for a JSON request: what this change ended is closed first.
                            Box::pin(crate::ports_runtime::close_ended(&app)).await;
                        }
                        crate::middleware::inspection::finish(&reply_ctx).await?;
                        crate::middleware::inspection::headers(&mut response, &reply_ctx)?;
                    }
                    Ok(response)
                }
                .await;
                match outcome {
                    Ok(response) => finish(response, &request_id),
                    Err(error) => error.into_response_for(&request_id),
                }
            }
        };
        self.add(route, on(filter(route.method), serve))
    }

    /// Serves `endpoint`, a WebSocket, with a handler that is given the upgrade once the request is read:
    /// who is calling and the query are read as for any request, and the handler decides, with the
    /// socket in its hand, whether and how it is let in.
    pub fn on_socket<E, F, Fut>(&mut self, endpoint: E, handler: F) -> &mut Self
    where
        E: Endpoint,
        F: Fn(Ctx, E::Query, WebSocketUpgrade) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = Response> + Send + 'static,
    {
        let _ = endpoint;
        let route = E::ROUTE;
        let serve = move |State(app): State<App>, request: Request| {
            let handler = handler.clone();
            async move {
                let request_id = orch_types::new_id().to_string();
                let (mut parts, _body) = request.into_parts();
                let outcome: ApiResult<Response> = async {
                    let ctx = Box::pin(open(&app, route, &mut parts, &request_id)).await?;
                    let query: E::Query = read_query(&route, parts.uri.query())?;
                    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &app)
                        .await
                        .map_err(|rejection| {
                            ApiError::new(ErrorCode::RequestInvalid, rejection.to_string())
                        })?;
                    Ok(handler(ctx, query, upgrade).await)
                }
                .await;
                match outcome {
                    Ok(response) => finish(response, &request_id),
                    Err(error) => error.into_response_for(&request_id),
                }
            }
        };
        self.add(route, on(filter(route.method), serve))
    }

    fn add(&mut self, route: Route, method_router: axum::routing::MethodRouter<App>) -> &mut Self {
        let router = std::mem::take(&mut self.router);
        self.router = router.route(route.path, method_router);
        self.bound.push(route);
        self
    }

    /// The requests that have a handler.
    #[must_use]
    pub fn bound(&self) -> &[Route] {
        &self.bound
    }

    /// The router, for mounting under the API's base path. A path nobody
    /// declared is answered as `not_found`, in the same form as any error.
    pub fn into_router(self) -> axum::Router<App> {
        self.router.fallback(|| async {
            ApiError::not_found().into_response_for(&orch_types::new_id().to_string())
        })
    }
}
