//! HTTP registration with permission and token metadata.
//!
//! A request is served only if the contract declares it. [`Registry::on`]
//! takes a declared endpoint and a handler whose arguments and result are
//! that endpoint's own query, body and reply types, so a handler cannot
//! read or answer anything else than what clients were generated from.
//! Around the handler, the same for every request: an identifier, who is
//! calling and whether that is enough for the endpoint, the query and the
//! body read strictly, and failures answered as problems.

use crate::key_operations::resolver;

use crate::key_operations::resolver::DataSlot;

use crate::key_operations::resolver::Disclosure;

use crate::key_operations::resolver::DisclosureState;

use crate::key_operations::resolver::OperationData;

use crate::key_operations::resolver::OperationRequest;

use crate::key_operations::activity;

use crate::key_operations::admission;

use crate::middleware::authentication;

use crate::middleware::authentication::Authed;

use crate::middleware::authentication::Caller;

use crate::middleware::authorization;

use crate::middleware::idempotency;

use crate::middleware::idempotency::Claim;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::state::App;

use axum::body::Body;

use axum::extract::ws::WebSocketUpgrade;

use axum::extract::FromRequestParts;

use axum::extract::RawPathParams;

use axum::extract::Request;

use axum::extract::State;

use axum::response::IntoResponse;

use axum::response::Response;

use axum::routing::MethodFilter;

use axum::routing::on;

use chrono::DateTime;

use chrono::Utc;

use http::HeaderMap;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use orch_api_contract::common::CSRF_HEADER;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::FieldFault;

use orch_api_contract::common::MAX_BODY_BYTES;

use orch_api_contract::common::MAX_JSON_BYTES;

use orch_api_contract::common::Problem;

use orch_api_contract::common::REQUEST_ID_HEADER;

use orch_api_contract::routes::Access;

use orch_api_contract::routes::Endpoint;

use orch_api_contract::routes::Method;

use orch_api_contract::routes::Route;

use orch_api_contract::routes::Shape;

use orch_api_contract::surface::HttpSurface;

use orch_types::Id;

use serde::Serialize;

use serde::de::DeserializeOwned;

use std::future::Future;

use std::net::IpAddr;

use std::pin::Pin;

use std::sync::Arc;

pub use crate::key_operations::admission::Prepared as OperationPlan;
/// A typed resolver may borrow the current context/connection/caller across its reads.
pub type OperationFuture<'a> = Pin<Box<dyn Future<Output = ApiResult<OperationPlan>> + Send + 'a>>;

/// The same effect-free producer is invoked at request preparation and on the fresh
/// mutation transaction. Inputs are owned decoded values; no Serialize/Clone bound is added.
pub type OperationResolver<Q, B> =
    for<'a> fn(&'a Ctx, &'a mut orch_db::Conn, &'a orch_authz::Caller, Q, B) -> OperationFuture<'a>;

/// A producer whose ordinary cold path genuinely has no declared operation yet.
/// None installs no proof and is never accepted for a modern scoped credential.
pub type OptionalOperationFuture<'a> =
    Pin<Box<dyn Future<Output = ApiResult<Option<OperationPlan>>> + Send + 'a>>;

pub type OptionalOperationResolver<Q, B> = for<'a> fn(
    &'a Ctx,
    &'a mut orch_db::Conn,
    &'a orch_authz::Caller,
    Q,
    B,
) -> OptionalOperationFuture<'a>;

pub enum ConfiguredOperation<Q, B> {
    Required(OperationResolver<Q, B>),
    Optional(OptionalOperationResolver<Q, B>),
    SnapshotRequired(OperationResolver<Q, B>),
    SnapshotOptional(OptionalOperationResolver<Q, B>),
}

impl<Q, B> ConfiguredOperation<Q, B> {
    pub fn snapshot_read(self) -> bool {
        matches!(self, Self::SnapshotRequired(_) | Self::SnapshotOptional(_))
    }
}

impl<Q, B> Copy for ConfiguredOperation<Q, B> {}

impl<Q, B> Clone for ConfiguredOperation<Q, B> {
    fn clone(&self) -> Self {
        *self
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RawInput {
    NoBodyRead,
    ChangingQuery,
    ChangingBody,
}

/// A trusted initial preparation may wait for an existing external refresh producer.
/// It receives no connection and is never retained or invoked by the transaction guard.
pub type OperationInitialFuture<'a> = Pin<Box<dyn Future<Output = ApiResult<()>> + Send + 'a>>;

/// Check known scope/ordinary authority on a short-lived connection, release it,
/// then perform only the operation's existing safe initial refresh/preparation.
pub type OperationInitial<Q, B> =
    for<'a> fn(&'a Ctx, &'a orch_authz::Caller, Q, B) -> OperationInitialFuture<'a>;

/// Existing original authentication data, opaque when a current context is logged.
#[derive(Clone)]
struct AuthenticatedUser(Option<Arc<Authed>>);

impl std::fmt::Debug for AuthenticatedUser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthenticatedUser([opaque])")
    }
}

#[derive(Clone, Default)]
struct CallerCeiling(Option<orch_authz::Caller>);

impl std::fmt::Debug for CallerCeiling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CallerCeiling([opaque])")
    }
}

/// What a handler is given about its request, beside the query and body.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// Private in-process Ask boundary, never read from a transport header.
    pub ask_read: Option<Arc<crate::ask::tools::ReadRequest>>,
    /// Actual HTTP mount; internal tool/machine projections have none.
    pub surface: Option<HttpSurface>,
    /// One logical browser opening, optionally shared with its live subscription.
    pub opening: Id,
    /// Whether this whitelisted reader explicitly requested deleted content.
    pub inspect_deleted: bool,
    /// The exact retained thing authorized before its stored payload is built.
    pub inspection: Option<crate::middleware::inspection::Info>,
    pub snapshot_inspection: Arc<crate::middleware::inspection::Snapshot>,
    /// The shared state.
    pub app: App,
    /// Who is calling.
    pub caller: Caller,
    /// Original authentication data remains an immutable credential ceiling when a
    /// same-request checkpoint updates the current user. This carries no admission.
    authenticated_user: AuthenticatedUser,
    /// Trusted authority admitted at this operation's current shared checkpoint.
    /// Never accepted from a request body, serialized, or retained for a later checkpoint.
    admitted_caller: Option<orch_authz::Caller>,
    /// Restored initial-phase restrictions, never target admission or a replacement origin.
    operation_ceiling: CallerCeiling,
    /// Shared by request/reply clones; a later guarded checkpoint may only narrow disclosure.
    key_receipt: Arc<DisclosureState>,
    /// Original bounded input and typed producer, private to this request.
    operation_request: Option<OperationRequest>,
    topic_creation: Option<Arc<crate::key_operations::creation_stage::TopicPrefix>>,
    /// Prepared read operands; guarded mutations use the Guard's own snapshot.
    operation_data: Arc<DataSlot>,
    /// Immutable bounded native initial operand, never replaced by a guarded resolver.
    pub operation_initial: Arc<tokio::sync::OnceCell<OperationData>>,
    /// Immutable opened association; current permission/target admission is always rebuilt.
    retained_source: Arc<crate::key_operations::resolver::RetainedSource>,
    /// Final transport lifetime and immutable native-source checks, independent of replanning.
    pub publication: Arc<crate::key_operations::publication::State>,
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
    /// Retain this authenticated entry's restrictions for a native preparation source.
    /// This is not a complete operation capture or a business-effect checkpoint.
    pub async fn capture_http_preparation(
        &self,
        conn: &mut orch_db::Conn,
        caller: &orch_authz::Caller,
    ) -> ApiResult<orch_authz::http_origin::HttpPreparation> {
        crate::key_operations::native::http_origin::capture_preparation(self, conn, caller).await
    }
    /// Check and retain the same raw/key minimum after the native preparation await.
    pub async fn current_http_preparation(
        &self,
        conn: &mut orch_db::Conn,
        origin: &orch_authz::http_origin::HttpPreparation,
    ) -> ApiResult<()> {
        crate::key_operations::native::http_origin::current_preparation(self, conn, origin).await
    }
    /// Origin for an HTTP protocol continuation. Internal tools have no fabricated HTTP plane.
    pub fn http_surface(&self) -> ApiResult<HttpSurface> {
        self.surface
            .ok_or_else(|| ApiError::internal("protocol continuation has no HTTP origin"))
    }

    /// A context for a tool's projections; authority for its writes is checked separately in its transaction.
    pub fn for_tool(app: App, caller: Caller, route: Route, id: Id) -> Self {
        let authenticated_user = AuthenticatedUser(match &caller {
            Caller::User(user) => Some(Arc::clone(user)),
            Caller::Anonymous => None,
        });
        let now = app.clock.now();
        Self {
            ask_read: None,
            surface: None,
            opening: orch_types::new_id(),
            inspect_deleted: false,
            inspection: None,
            snapshot_inspection: Arc::default(),
            app,
            caller,
            authenticated_user,
            admitted_caller: None,
            operation_ceiling: CallerCeiling::default(),
            key_receipt: Arc::default(),
            operation_request: None,
            topic_creation: None,
            operation_data: Arc::default(),
            operation_initial: Arc::default(),
            retained_source: Arc::default(),
            publication: Arc::default(),
            request_id: format!("tool:{id}"),
            now,
            headers: HeaderMap::new(),
            remote: None,
            route,
            params: vec![("id".to_owned(), id.to_string())],
        }
    }

    /// Carries the existing caller admitted with freshly resolved facts at this operation's
    /// current checkpoint. Later transactions, emissions and jobs must resolve and admit anew;
    /// this context is not a saved target-path authority. Mutations retain their action guard.
    pub fn with_admitted_caller(mut self, caller: orch_authz::Caller) -> ApiResult<Self> {
        let user = self.user()?;
        if caller.grant().is_some()
            && !caller
                .admitted_facets()
                .is_some_and(|facets| !facets.is_empty())
            && !crate::key_operations::admission::checked_empty_for(&self.route, &caller)?
        {
            return Err(ApiError::internal(
                "a key caller needs current operation admission",
            ));
        }
        let origin_matches = authorization::same_credential_origin(user, &caller);
        if !origin_matches {
            return Err(ApiError::internal(
                "an admitted caller does not match its authenticated credential origin",
            ));
        }
        self.admitted_caller = Some(caller);
        Ok(self)
    }

    pub fn admitted_caller(&self) -> Option<&orch_authz::Caller> {
        self.admitted_caller.as_ref()
    }

    pub fn operation_ceiling(&self) -> Option<&orch_authz::Caller> {
        self.operation_ceiling.0.as_ref()
    }

    pub fn with_operation_ceiling(mut self, caller: orch_authz::Caller) -> ApiResult<Self> {
        if !admission::same_response_origin(&self, &caller)? {
            return Err(ApiError::internal(
                "initial preparation changed its HTTP origin",
            ));
        }
        let restricted = match self.operation_ceiling.0.take() {
            Some(accepted) => caller
                .intersect_restrictions(&accepted)
                .map_err(ApiError::internal)?,
            None => caller,
        };
        self.publication.retain_deadline(
            restricted
                .grant()
                .and_then(orch_authz::TokenGrant::expires_at),
        )?;
        self.operation_ceiling.0 = Some(restricted);
        Ok(self)
    }

    /// The native raw decoder installs only its factual immutable-input producer.
    /// No caller or target admission is installed until normal complete preparation.
    pub fn with_operation_request(mut self, request: OperationRequest) -> Self {
        self.operation_request = Some(request);
        self
    }

    /// Final serialization uses a receipt until a completed response check grants content.
    /// Pending producers still need their optional read checkpoint; only Denied is terminal.
    pub fn key_receipt_required(&self) -> bool {
        self.disclosure() != Disclosure::Full
    }

    pub fn disclosure(&self) -> Disclosure {
        self.key_receipt.current()
    }
    pub fn checkpoint_disclosure(&self, value: Disclosure) {
        self.key_receipt.checkpoint(value);
    }
    pub fn complete_disclosure(&self, value: Disclosure) {
        self.key_receipt.complete(value);
    }

    /// A native refusal replaces the unpublished success shape. Give its independently
    /// declared Read projection a private disclosure state; identity/ceilings and final
    /// transport source/deadline state remain the original request's.
    pub fn for_refusal_projection(&self) -> Self {
        let mut child = self.clone();
        child.key_receipt = Arc::default();
        child.checkpoint_disclosure(Disclosure::Pending);
        child
    }
    /// A completed replay has a new credential/read purpose, independently of spent effects.
    /// It keeps this request's real identity, ceilings, source guards and transport lifetime.
    pub fn for_completed_replay_projection(&self) -> Self {
        let mut child = self.clone();
        child.key_receipt = Arc::default();
        child.checkpoint_disclosure(Disclosure::Pending);
        child
    }
    pub fn operation_request(&self) -> Option<&OperationRequest> {
        self.operation_request.as_ref()
    }
    pub fn clear_operation_data(&self) -> ApiResult<()> {
        self.operation_data.replace(OperationData::default())
    }
    pub fn publish_operation_data(&self, data: OperationData) -> ApiResult<()> {
        self.operation_data.replace(data)
    }

    /// Operands captured by this request's current read preparation. Mutation handlers
    /// use Guard::operation_data after beginning their normal guarded transaction.
    pub fn operation_data<T: Send + Sync + 'static>(&self) -> ApiResult<Arc<T>> {
        self.operation_data.get()
    }

    pub fn operation_data_snapshot(&self) -> ApiResult<OperationData> {
        self.operation_data.snapshot()
    }

    pub fn snapshot_read(&self) -> bool {
        self.operation_request
            .as_ref()
            .is_some_and(OperationRequest::snapshot_read)
    }

    /// Retains a real opened association once, or checks the identical value on later use.
    pub fn retain_source<T: Send + Sync + PartialEq + 'static>(&self, value: T) -> ApiResult<()> {
        self.retained_source.retain(value)
    }

    pub fn retained_source<T: Send + Sync + 'static>(&self) -> ApiResult<Arc<T>> {
        self.retained_source.get()
    }

    /// Distinguishes a fresh unopened request from corrupt retained source data.
    pub fn optional_retained_source<T: Send + Sync + 'static>(&self) -> ApiResult<Option<Arc<T>>> {
        self.retained_source.optional()
    }

    /// Re-resolve the shared read producer on its actual read connection/snapshot.
    /// Mutation connections already run this producer inside the ordinary action guard.
    pub async fn operation_principal(
        &self,
        conn: &mut orch_db::Conn,
    ) -> ApiResult<orch_authz::Principal> {
        if self.snapshot_read() {
            return Ok(Box::pin(transaction_guard::check_snapshot_read(self, conn))
                .await?
                .into_principal());
        }
        if transaction_guard::changes(self)
            && orch_db::queries::transaction_guard::in_transaction(conn)
        {
            // Action mode owns its existing epoch/credential lock order.
            return authorization::principal(self, conn).await;
        }
        let http = if self.surface.is_some() {
            Some(
                transaction_guard::current_http_identity(
                    self,
                    conn,
                    transaction_guard::HttpIdentityMode::Read,
                )
                .await?,
            )
        } else {
            None
        };
        let mut principal = match &http {
            Some(identity) => {
                let principal =
                    authorization::load_principal(conn, identity.caller.clone(), identity.now)
                        .await?;
                crate::sign_in::second_step::hold(conn, &identity.user, principal).await?
            }
            None => authorization::principal(self, conn).await?,
        };
        // A static legacy reader has no earlier operation admission. Restore its
        // original token boundary too, so a new stored operation policy cannot be
        // missed by the ordinary early return below.
        // An admitted caller was already restored by authorization::principal.
        if http.is_none()
            && self.admitted_caller().is_none()
            && principal.caller().grant().is_some()
        {
            let caller = restore_operation_caller(self, conn, principal.caller()).await?;
            principal = authorization::load_principal(
                conn,
                caller,
                orch_types::Time::new(self.app.clock.now()),
            )
            .await?;
            principal = crate::sign_in::second_step::hold(conn, self.user()?, principal).await?;
        }
        let scoped = principal
            .caller()
            .grant()
            .is_some_and(|grant| grant.operation_scope().is_some())
            || self
                .admitted_caller()
                .and_then(orch_authz::Caller::grant)
                .is_some_and(|grant| grant.operation_scope().is_some());
        if self.operation_request.is_none() && !scoped {
            return Ok(principal);
        }
        let caller = match self.admitted_caller() {
            Some(accepted) if accepted.grant().is_some() => accepted
                .intersect_restrictions(principal.caller())
                .map_err(ApiError::internal)?,
            _ => principal.caller().clone(),
        };
        // Final typed data must see the actual current HTTP mode/user, not a
        // historical browser-admin context lent to a cached response body.
        let checkpoint;
        let preparing = match &http {
            Some(identity) => {
                checkpoint =
                    self.for_operation_checkpoint(identity.user.clone(), identity.now.get());
                &checkpoint
            }
            None => self,
        };
        let Some(admission) = resolver::resolve(preparing, conn, &caller).await? else {
            resolver::ordinary_unclassified(&caller)?;
            return Ok(principal);
        };
        let admission = admission.complete()?;
        self.checkpoint_disclosure(admission.disclosure);
        self.publish_operation_data(admission.data)?;
        // Keep the returned ordinary evaluation context aligned with the fresh
        // preparation that produced its typed data. A later response boundary must
        // validate/rebuild that context, not retime old protected payloads.
        let evaluated_at = http.as_ref().map_or_else(
            || orch_types::Time::new(self.app.clock.now()),
            |identity| identity.now,
        );
        let principal = authorization::load_principal(conn, admission.caller, evaluated_at).await?;
        let user = match &http {
            Some(identity) => &identity.user,
            None => self.user()?.as_ref(),
        };
        crate::sign_in::second_step::hold(conn, user, principal).await
    }

    pub fn topic_creation_prefix(
        &self,
    ) -> Option<&Arc<crate::key_operations::creation_stage::TopicPrefix>> {
        self.topic_creation.as_ref()
    }

    fn with_topic_creation_prefix(
        mut self,
        prefix: crate::key_operations::creation_stage::TopicPrefix,
    ) -> Self {
        self.checkpoint_disclosure(Disclosure::Pending);
        self.topic_creation = Some(Arc::new(prefix));
        self
    }

    /// The resolver observes the guarded identity/time, without an earlier admission
    /// being mistaken for proof at this new checkpoint. Original input remains unchanged.
    pub fn for_operation_checkpoint(&self, user: Authed, now: DateTime<Utc>) -> Self {
        let mut current = self.clone();
        current.caller = Caller::User(Arc::new(user));
        current.now = now;
        current.admitted_caller = None;
        current
    }

    /// Concurrent native read attempts own their preparation slot. Transport
    /// provenance, immutable initial operands and monotone publication cuts stay
    /// shared, but another source cannot replace this attempt's selected value.
    pub fn for_read_preparation(&self) -> Self {
        let mut current = self.clone();
        // current_http_identity restores these accepted restrictions first;
        // for_operation_checkpoint then clears their stale target admission.
        current.operation_data = Arc::default();
        current
    }

    /// A read's auxiliary audit checks the same current identity and restrictions,
    /// but its provisional operands and disclosure are not the response's projection.
    /// Final publication still checks the original selected response independently.
    pub fn for_auxiliary_check(&self) -> Self {
        let mut current = self.clone();
        current.key_receipt = Arc::default();
        current.operation_data = Arc::default();
        current
    }

    /// A metadata discovery may take the native locks but cannot publish its
    /// provisional operands or disclosure into the eventual action/response context.
    pub fn for_effect_discovery(&self, user: Authed, now: DateTime<Utc>) -> Self {
        let mut discovery = self.for_operation_checkpoint(user, now);
        discovery.key_receipt = Arc::default();
        discovery.operation_data = Arc::default();
        // The authenticated opened association is immutable and remains shared.
        discovery
    }

    /// The original HTTP authentication, separate from a refreshed current user.
    pub fn authenticated_user(&self) -> ApiResult<&Arc<Authed>> {
        self.authenticated_user
            .0
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired, "sign in first"))
    }

    /// Retains the reader's identity while checking one requested child separately.
    pub fn for_id(&self, id: Id) -> Self {
        let mut child = self.clone();
        for (name, value) in &mut child.params {
            if name == "id" {
                *value = id.to_string();
            }
        }
        child.inspection = None;
        child.snapshot_inspection = Arc::default();
        // A proof for the previous object is not admission for this child.
        child.admitted_caller = None;
        child.key_receipt = Arc::default();
        child.operation_request = None;
        child.topic_creation = None;
        child.operation_data = Arc::default();
        child.operation_initial = Arc::default();
        child.retained_source = Arc::default();
        child
    }
    pub fn with_snapshot_inspection(mut self) -> ApiResult<Self> {
        if let Some(info) = self.snapshot_inspection.get()? {
            self.inspection = info;
        }
        Ok(self)
    }

    /// The signed-in caller; refused with `auth.required` when there is none.
    pub fn user(&self) -> ApiResult<&Arc<Authed>> {
        match &self.caller {
            Caller::User(authed) => Ok(authed),
            Caller::Anonymous => Err(ApiError::new(ErrorCode::AuthRequired, "sign in first")),
        }
    }

    /// The path parameter `name` as it was written, where the path has it (nothing is logged).
    pub fn param_if_any(&self, name: &str) -> Option<&str> {
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
fn cross_root_move(route: &Route) -> bool {
    matches!(
        route.name,
        "EnvironmentMoveOut"
            | "ProviderMoveOut"
            | "ChatMoveOut"
            | "McpServerMoveOut"
            | "AgentMoveOut"
            | "TemplateMoveOut"
            | "MountMoveOut"
            | "SkillMoveOut"
            | "SkillDraftMoveOut"
            | "HostMoveOut"
            | "MachineMoveOut"
    )
}

/// Whether this endpoint can change authority, including a rolled-back trial.
pub(crate) fn changes_authority(route: &Route) -> bool {
    cross_root_move(route)
        || matches!(
            route.name,
            // Pages, roles and previews that write and roll back a trial page.
            "NodeRoleCreate" | "RoleUpdate" | "RoleDelete" |
        "PermissionPageUpdate" | "PermissionPagePreview" | "SharePreviewCreate" |
        "NodeDefaultsUpdate" | "NodeDefaultsPreview" |
        "AdminDefaultsUpdate" | "AdminDefaultsPreview" |
        "NodeTransfer" | "ObjectTransfer" | "ObjectHomeSet" |
        // Memberships, groups, suspensions, service identities and invitations.
        "OrgMemberRemove" | "OrgMemberMove" | "OrgMemberSuspend" | "OrgMemberResume" |
        "OrgOutsiderRemove" |
        "OrgGroupCreate" | "GroupUpdate" | "GroupDelete" | "GroupMemberAdd" | "GroupMemberRemove" |
        "GroupHolderAdd" | "GroupHolderRemove" |
        "OrgServiceAccountCreate" | "ServiceAccountDelete" |
        "ServiceAccountTokenCreate" | "ServiceAccountTokenDelete" | "InvitationAccept" |
        // Another person's standing, or a token's admission to a shared org.
        "OrgTokenRefuse" | "AdminTokenDelete" | "AdminUserTokenDelete" |
        "AdminUserSessionDelete" | "AdminUserSessionsDelete" |
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
    !cross_root_move(route)
        && !matches!(
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
    key_router: axum::Router<App>,
    bound: Vec<Route>,
    creations: Arc<idempotency::Creations>,
    creation_claims: Arc<idempotency::Claims>,
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

pub(crate) fn read_query<Q: DeserializeOwned>(route: &Route, query: Option<&str>) -> ApiResult<Q> {
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

pub(crate) fn decode_body<B: DeserializeOwned>(route: &Route, bytes: &[u8]) -> ApiResult<B> {
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
    typed_operation: bool,
    deferred_creation: bool,
) -> ApiResult<Ctx> {
    let ask_read = parts
        .extensions
        .get::<Arc<crate::ask::tools::ReadRequest>>()
        .cloned();
    if let Some(origin) = ask_read.as_ref().and_then(|read| read.http_origin()) {
        // Preserve the exact admitted Ask transport; native owner reads have none to borrow.
        parts.headers = origin.source.headers.clone();
    }
    let surface = parts.extensions.get::<HttpSurface>().copied();
    let native_read = ask_read
        .as_ref()
        .is_some_and(|read| read.native_identity().is_some());
    if let Some(surface) = surface {
        if native_read {
            return Err(ApiError::internal(
                "a native read cannot claim an HTTP surface",
            ));
        }
        authentication::on_surface(surface, &parts.headers)?;
    } else if !native_read {
        return Err(ApiError::internal(
            "HTTP request has no server-selected surface",
        ));
    }
    let http_surface =
        || surface.ok_or_else(|| ApiError::internal("a native read has no HTTP authenticator"));
    let now = app.clock.now();
    let inspect_deleted = match crate::middleware::inspection::requested(&route, parts.uri.query())
    {
        Ok(requested) => requested,
        Err(error) => {
            // Preserve this existing early validation response, while attributing it
            // only when the supplied key and its account actually authenticate.
            // Public routes ignore credentials, and invalid keys never acquire a log identity.
            if route.access.signed_in()
                && authentication::bearer(&parts.headers)
                    .is_some_and(|token| token.starts_with(authentication::API_TOKEN_PREFIX))
                && let Ok(Caller::User(authed)) =
                    authentication::authenticate_on(http_surface()?, app, &parts.headers, now).await
                && let authentication::Via::Token { token_id, .. } = authed.via
            {
                activity::authenticated(activity::KeyOrigin {
                    key_id: token_id,
                    account_id: authed.id(),
                });
            }
            return Err(error);
        }
    };
    let caller = match route.access {
        Access::Public => Caller::Anonymous,
        Access::Cookie => {
            if !parts.headers.contains_key(CSRF_HEADER) {
                return Err(ApiError::new(
                    ErrorCode::AuthCsrf,
                    "the request lacks its header",
                ));
            }
            Caller::Anonymous
        }
        Access::User | Access::Session | Access::Permission(_) | Access::Rule(_) => {
            let caller = if let Some(read) = &ask_read {
                let mut user = if let Some(origin) = read.http_origin() {
                    origin.source.user()?.as_ref().clone()
                } else {
                    read.native_user(app).await?
                };
                user.as_admin = false;
                Caller::User(Arc::new(user))
            } else {
                authentication::authenticate_on(http_surface()?, app, &parts.headers, now).await?
            };
            let Caller::User(authed) = &caller else {
                return Err(ApiError::new(ErrorCode::AuthRequired, "sign in first"));
            };
            if let authentication::Via::Token { token_id, .. } = authed.via {
                activity::authenticated(activity::KeyOrigin {
                    key_id: token_id,
                    account_id: authed.id(),
                });
            }
            if matches!(route.access, Access::Session)
                && !authed.is_session()
                && !matches!(
                    &authed.via,
                    authentication::Via::Token {
                        operation_scope: Some(_),
                        ..
                    }
                )
            {
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
    let authenticated_user = AuthenticatedUser(match &caller {
        Caller::User(user) => Some(Arc::clone(user)),
        Caller::Anonymous => None,
    });
    let mut ctx = Ctx {
        ask_read: ask_read.clone(),
        surface,
        opening: orch_types::new_id(),
        inspect_deleted,
        inspection: None,
        snapshot_inspection: Arc::default(),
        app: app.clone(),
        caller,
        authenticated_user,
        admitted_caller: None,
        operation_ceiling: CallerCeiling::default(),
        key_receipt: Arc::default(),
        operation_request: None,
        topic_creation: None,
        operation_data: Arc::default(),
        operation_initial: Arc::default(),
        retained_source: Arc::default(),
        publication: Arc::default(),
        request_id: request_id.to_owned(),
        now,
        headers: parts.headers.clone(),
        remote: remote(app, parts),
        route,
        params,
    };
    if let Some(origin) = &ask_read {
        ctx = ctx.with_operation_ceiling(origin.ceiling())?;
    }
    if let Ok(user) = ctx.user()
        && let authentication::Via::Token { expires_at, .. } = &user.via
    {
        ctx.publication.retain_deadline(*expires_at)?;
    }
    let rule_key = ctx.user().is_ok_and(|user| {
        matches!(
            &user.via,
            authentication::Via::Token {
                operation_scope: Some(_),
                ..
            }
        )
    });
    if deferred_creation {
        return Ok(ctx);
    }
    if (rule_key || ask_read.is_some()) && !typed_operation {
        ctx = admit_implicit_open(ctx).await?;
    }
    // Legacy/browser permissions keep their original pre-body order. A registered
    // rule-key producer needs validated input before scope admission can precede
    // the very same ordinary permission/inspection checks.
    if rule_key && typed_operation {
        return Ok(ctx);
    }
    authorize_open(ctx, parts.uri.query()).await
}

/// The existing implicit path gate, also restored for a new deferred creation claim.
async fn admit_implicit_open(ctx: Ctx) -> ApiResult<Ctx> {
    let mut conn = ctx.app.db.acquire().await?;
    let caller = authorization::caller_of(ctx.user()?)?;
    let current = authorization::load_principal(&mut conn, caller, ctx.app.clock.time()).await?;
    let admission = crate::key_operations::admission::at_path(&ctx, &mut conn, current.caller())
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the operation needs its resolved scope admission",
            )
        })?;
    crate::key_operations::admission::install(ctx, admission)
}

async fn authorize_open(mut ctx: Ctx, query: Option<&str>) -> ApiResult<Ctx> {
    ctx.opening = crate::middleware::admin_openings::nonce(&ctx)?;
    if ctx.inspect_deleted {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        ctx.inspection =
            crate::middleware::inspection::prepare_query(&ctx, &mut conn, &principal, query)
                .await?;
    }
    if let Access::Permission(permission) = ctx.route.access {
        authorization::require_on_path(&ctx, permission).await?;
        // A granted use of a thing in an org is when its caller was last seen there.
        authorization::note_use(&ctx).await;
    }
    Ok(ctx)
}

/// Restore only the same accepted credential boundary before/after an external wait.
/// Returned callers own their facts; nothing retains this borrowed connection.
pub(crate) async fn restore_operation_caller(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    accepted: &orch_authz::Caller,
) -> ApiResult<orch_authz::Caller> {
    if !admission::same_response_origin(ctx, accepted)? {
        return Err(ApiError::internal(
            "initial preparation changed credential origin",
        ));
    }
    let identity = transaction_guard::current_http_identity(
        ctx,
        conn,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    transaction_guard::guard_user(&identity.user)?;
    let current = orch_authz::CapturedCaller::capture(accepted)
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the preparation credential is no longer valid",
            )
        })?;
    let current = identity
        .caller
        .intersect_restrictions(&current)
        .map_err(ApiError::internal)?;
    let principal = authorization::load_principal(conn, current, ctx.app.clock.time()).await?;
    let principal = crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
    identity.check_deadline(&ctx.app)?;
    if !principal
        .caller()
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the initial credential has expired",
        ));
    }
    Ok(principal.caller().clone())
}

/// The native body stage has completed; full declaration and original ordinary opening gates
/// precede existing-source IO or application. Only the immutable final resolver is retained.
pub(crate) async fn prepare_registered_raw_body<Q>(ctx: Ctx, query: Option<&str>) -> ApiResult<Ctx>
where
    Q: DeserializeOwned + Send + 'static,
{
    let ctx = prepare_registered_operation::<Q, ()>(ctx, query, &[], None).await?;
    if ctx.user().is_ok_and(|user| {
        matches!(
            user.via,
            authentication::Via::Token {
                operation_scope: Some(_),
                ..
            }
        )
    }) {
        authorize_open(ctx, query).await
    } else {
        Ok(ctx)
    }
}

/// Initial preparation is deliberately outside the connection used by final resolution.
/// The guard stores only OperationRequest's final resolver, so it cannot run this phase.
async fn prepare_registered_operation<Q, B>(
    mut ctx: Ctx,
    query: Option<&str>,
    body: &[u8],
    initial: Option<OperationInitial<Q, B>>,
) -> ApiResult<Ctx>
where
    Q: DeserializeOwned + Send + 'static,
    B: DeserializeOwned + Send + 'static,
{
    let Ok(user) = ctx.user() else {
        return Ok(ctx);
    };
    let mut accepted = match ctx.admitted_caller() {
        Some(caller) => caller.clone(),
        None => authorization::caller_of(user)?,
    };
    if let Some(initial) = initial {
        accepted = {
            let mut conn = ctx.app.db.acquire().await?;
            restore_operation_caller(&ctx, &mut conn, &accepted).await?
        }; // The connection is returned before any external refresh is awaited.
        ctx = ctx.with_operation_ceiling(accepted.clone())?;
        let mut initial_ctx =
            ctx.for_operation_checkpoint(ctx.user()?.as_ref().clone(), ctx.app.clock.now());
        initial_ctx.operation_request = None;
        initial_ctx.clear_operation_data()?;
        let query = read_query::<Q>(&ctx.route, query)?;
        let body = decode_body::<B>(&ctx.route, body)?;
        initial(&initial_ctx, &accepted, query, body).await?;
    }
    let mut conn = ctx.app.db.acquire().await?;
    let caller = if initial.is_some() {
        // This restores current policy and intersects it with the pre-refresh ceiling.
        restore_operation_caller(&ctx, &mut conn, &accepted).await?
    } else {
        // Preserve the existing no-initial-phase path and its ordinary caller handling.
        let principal = authorization::load_principal(
            &mut conn,
            accepted,
            orch_types::Time::new(ctx.app.clock.now()),
        )
        .await?;
        principal.caller().clone()
    };
    ctx.now = ctx.app.clock.now();
    if initial.is_some() {
        ctx = ctx.with_operation_ceiling(caller.clone())?;
    }
    if ctx.snapshot_read() {
        // The initial callback ran once at entry. Only the final factual selector
        // belongs to the watched read snapshot and any retry of that snapshot.
        return Ok(ctx);
    }
    let admission = resolver::resolve(&ctx, &mut conn, &caller).await?;
    drop(conn);
    match admission {
        Some(resolver::Resolution::Complete(admission)) => {
            crate::key_operations::admission::install(ctx, admission)
        }
        Some(resolver::Resolution::TopicCreationPrefix(prefix)) => {
            Ok(ctx.with_topic_creation_prefix(prefix))
        }
        None => {
            resolver::ordinary_unclassified(&caller)?;
            Ok(ctx)
        }
    }
}

fn finish(mut response: Response, request_id: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    response
}

fn key_call_outcome(outcome: &ApiResult<Response>) -> (u16, Option<ErrorCode>) {
    match outcome {
        Ok(response) => (response.status().as_u16(), None),
        Err(error) => (error.problem.status, Some(error.code())),
    }
}

async fn record_key_call(
    app: &App,
    call: &activity::Call,
    (status, code): (u16, Option<ErrorCode>),
) {
    if let Some(record) = call.finish(status, code)
        && let Err(error) = activity::record(app, record).await
    {
        // History is bounded operational bookkeeping, not another transaction.
        // A logging failure cannot turn a committed operation into a retry.
        tracing::warn!(
            code = error.code().as_str(),
            "key activity could not be recorded"
        );
    }
}

/// Catalogue coverage distinguishes a decided protocol/native body key from a missing binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreationCoverage {
    SharedHeader,
    NativeClientId,
    NativeHeader(&'static str),
    NotKeyed(&'static str),
    Undecided,
}

impl Registry {
    /// Inspect the same registration used by execution; absence alone is not an exemption.
    pub fn creation_coverage<E: Endpoint>(&self, _endpoint: E) -> ApiResult<CreationCoverage> {
        if self
            .creations
            .get::<E::Query, E::Body, E::Reply>(E::ROUTE.name)?
            .is_some()
        {
            return Ok(CreationCoverage::SharedHeader);
        }
        Ok(match E::ROUTE.idempotency {
            orch_api_contract::routes::Idempotency::Unspecified => CreationCoverage::Undecided,
            orch_api_contract::routes::Idempotency::ClientId => CreationCoverage::NativeClientId,
            orch_api_contract::routes::Idempotency::NativeHeader(reason) => {
                CreationCoverage::NativeHeader(reason)
            }
            orch_api_contract::routes::Idempotency::NotKeyed(reason) => {
                CreationCoverage::NotKeyed(reason)
            }
        })
    }
    /// No requests yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Native creation metadata and current same-operation replay, consumed by the one JSON seam.
    pub fn creation<E: Endpoint>(
        &mut self,
        _endpoint: E,
        binding: impl Into<idempotency::Registered<E::Query, E::Body, E::Reply>>,
    ) -> &mut Self {
        assert_eq!(
            E::ROUTE.method.as_str(),
            "POST",
            "only creating POST registrations have a creation binding"
        );
        assert_eq!(
            E::ROUTE.idempotency,
            orch_api_contract::routes::Idempotency::Unspecified,
            "an explicitly native-keyed or unkeyed route cannot acquire a header creation binding"
        );
        self.creations.insert(E::ROUTE.name, binding.into());
        self
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
        self.on_configured(endpoint, None, None, handler)
    }

    /// Register the endpoint's shared typed, effect-free operation producer. Its
    /// original validated input is decoded again by the same contract at each fresh
    /// guarded checkpoint; the existing handler and business writer remain unchanged.
    pub fn on_with_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        resolver: OperationResolver<E::Query, E::Body>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        self.on_configured(
            endpoint,
            None,
            Some(ConfiguredOperation::Required(resolver)),
            handler,
        )
    }

    /// Preserve an ordinary unclassified cold path without creating a key admission proof.
    pub fn on_with_optional_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        resolver: OptionalOperationResolver<E::Query, E::Body>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        self.on_configured(
            endpoint,
            None,
            Some(ConfiguredOperation::Optional(resolver)),
            handler,
        )
    }

    /// An effect-free reader resolves its native projection only inside its own
    /// database snapshot, rather than preparing it at entry and repeating it later.
    /// The response must pass the same watched current-identity read guard.
    pub fn on_snapshot_read_with_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        resolver: OperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint<Body = ()>,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, ()) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        assert_eq!(
            E::ROUTE.method,
            Method::GET,
            "a snapshot reader must be a GET"
        );
        self.on_configured(
            endpoint,
            None,
            Some(ConfiguredOperation::SnapshotRequired(resolver)),
            handler,
        )
    }

    /// The same snapshot-only preparation for a family preserving its ordinary
    /// unclassified legacy path. Missing modern admission still fails closed.
    pub fn on_snapshot_read_with_optional_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        resolver: OptionalOperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint<Body = ()>,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, ()) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        assert_eq!(
            E::ROUTE.method,
            Method::GET,
            "a snapshot reader must be a GET"
        );
        self.on_configured(
            endpoint,
            None,
            Some(ConfiguredOperation::SnapshotOptional(resolver)),
            handler,
        )
    }

    /// Run one trusted initial step outside retryable native read preparation,
    /// then resolve the optional producer only in its actual watched snapshot.
    /// The initial callback must admit its real known subjects and native gate
    /// before any report/refresh; its restored credential ceiling remains live.
    pub fn on_snapshot_read_with_optional_initial_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        initial: OperationInitial<E::Query, ()>,
        resolver: OptionalOperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint<Body = ()>,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, ()) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        assert!(
            E::ROUTE.method == Method::GET && matches!(E::ROUTE.body, Shape::None),
            "an initial snapshot reader is a GET without body operands"
        );
        self.on_configured(
            endpoint,
            Some(initial),
            Some(ConfiguredOperation::SnapshotOptional(resolver)),
            handler,
        )
    }

    /// A trusted effect-free POST preview selects from its original bounded JSON
    /// input in the actual watched snapshot. Writers keep their action registration.
    pub fn on_snapshot_body_read_with_optional_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        resolver: OptionalOperationResolver<E::Query, E::Body>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        assert!(
            E::ROUTE.method == Method::POST && matches!(E::ROUTE.body, Shape::Json(_)),
            "an effect-free body reader is an explicitly registered POST with JSON input"
        );
        self.on_configured(
            endpoint,
            None,
            Some(ConfiguredOperation::SnapshotOptional(resolver)),
            handler,
        )
    }

    /// Explicit effect-free POST planner with the contract's genuine absent body.
    /// It uses the same watched snapshot/current required-operation gate as other readers.
    pub fn on_snapshot_post_read_with_optional_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        resolver: OptionalOperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint<Body = ()>,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, ()) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        assert!(
            E::ROUTE.method == Method::POST && matches!(E::ROUTE.body, Shape::None),
            "an effect-free no-body planner is an explicitly registered POST without a body"
        );
        self.on_configured(
            endpoint,
            None,
            Some(ConfiguredOperation::SnapshotOptional(resolver)),
            handler,
        )
    }

    /// One native initial supplier followed by a watched effect-free POST body preview.
    /// Only its final typed factual selector can repeat with a fresh snapshot.
    pub fn on_snapshot_body_read_with_optional_initial_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        initial: OperationInitial<E::Query, E::Body>,
        resolver: OptionalOperationResolver<E::Query, E::Body>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        assert!(
            E::ROUTE.method == Method::POST && matches!(E::ROUTE.body, Shape::Json(_)),
            "an initial body preview is an explicitly registered POST with JSON input"
        );
        self.on_configured(
            endpoint,
            Some(initial),
            Some(ConfiguredOperation::SnapshotOptional(resolver)),
            handler,
        )
    }

    /// Preserve a producer's safe external refresh before final typed resolution.
    /// Initial preparation is never executed under a held registry connection or
    /// mutation transaction, and is not part of the guard's retained resolver.
    pub fn on_with_initial_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        initial: OperationInitial<E::Query, E::Body>,
        resolver: OperationResolver<E::Query, E::Body>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        self.on_configured(
            endpoint,
            Some(initial),
            Some(ConfiguredOperation::Required(resolver)),
            handler,
        )
    }

    /// Run an existing safe initial refresh before an optional typed producer.
    /// Unclassified modern-key requests still fail closed after the refresh.
    pub fn on_with_optional_initial_operation<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        initial: OperationInitial<E::Query, E::Body>,
        resolver: OptionalOperationResolver<E::Query, E::Body>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        E::Body: DeserializeOwned,
        E::Reply: Serialize,
        F: Fn(Ctx, E::Query, E::Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<R>> + Send + 'static,
        R: Into<Answer<E::Reply>>,
    {
        self.on_configured(
            endpoint,
            Some(initial),
            Some(ConfiguredOperation::Optional(resolver)),
            handler,
        )
    }

    fn on_configured<E, F, Fut, R>(
        &mut self,
        endpoint: E,
        initial: Option<OperationInitial<E::Query, E::Body>>,
        operation: Option<ConfiguredOperation<E::Query, E::Body>>,
        handler: F,
    ) -> &mut Self
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
        let creations = self.creations.clone();
        let creation_claims = self.creation_claims.clone();
        let serve = move |State(app): State<App>, request: Request| {
            let handler = handler.clone();
            let creations = creations.clone();
            let creation_claims = creation_claims.clone();
            async move {
                let request_id = orch_types::new_id().to_string();
                let (mut parts, body) = request.into_parts();
                let call = activity::Call::http(
                    route,
                    request_id.clone(),
                    orch_types::Time::new(app.clock.now()),
                    remote(&app, &parts),
                );
                let mut publication_ctx = None;
                let outcome: ApiResult<Response> = call
                    .scope(sent(async {
                        // Public requests still share this future's permission/inspection branches.
                        // Keep that common authorization state off the request's inline stack.
                        let creation = creations.get::<E::Query,E::Body,E::Reply>(route.name)?;
                        let deferred_creation = creation.is_some() && parts.headers.contains_key(orch_api_contract::common::IDEMPOTENCY_HEADER);
                        let mut ctx = Box::pin(open(
                            &app,
                            route,
                            &mut parts,
                            &request_id,
                            operation.is_some(),
                            deferred_creation,
                        ))
                        .await?;
                        publication_ctx = Some(ctx.clone());
                        let query: E::Query = read_query(&route, parts.uri.query())?;
                        let body_bytes = read_body_bytes(&route, body).await?;
                        let body: E::Body = decode_body(&route, &body_bytes)?;
                        ctx.now = app.clock.now();
                        let query_text = parts.uri.query().map(str::to_owned);
                        let mut proven_account = None;
                        let (claim,unassigned) = match &creation {
                            Some(binding) if deferred_creation && binding.principal.is_some() => {
                                let _ = idempotency::request_key(&ctx)?;
                                proven_account = (binding.principal.ok_or_else(||ApiError::internal("a public creation has no native proof"))?)(ctx.clone(),decode_body(&route,&body_bytes)?).await?;
                                creation_claims.begin_proven(&ctx,binding.kind,&parts.uri,&body_bytes,proven_account).await?
                            },
                            Some(binding) => (creation_claims.begin_continuing(&ctx,binding.kind,&parts.uri,idempotency::BodyIdentity::Json(&body_bytes),binding.resume).await?,None),
                            None => {creation_claims.check_other(&ctx,&parts.uri,&body_bytes).await?;(None,None)},
                        };
                        let (answer,guarded,replayed,reply_ctx) = match claim {
                            Some(Claim::Saved{account,record,status}) => {
                                crate::key_operations::activity::replayed();
                                let current_account = if creation.as_ref().is_some_and(|binding|binding.principal.is_some()) {
                                    proven_account.ok_or_else(||ApiError::internal("a completed public creation has no current proof"))?
                                } else {ctx.user()?.id()};
                                if account != current_account {return Err(ApiError::internal("the creation account differs from the proven account"));}
                                let reply_ctx = ctx.clone();
                                let body = if let Some(created) = record.outcome {
                                    (creation.as_ref().ok_or_else(||ApiError::internal("a completed creation has no native projector"))?.replay.ok_or_else(||ApiError::internal("a JSON creation has a raw replay binding"))?)(ctx,query,body,created).await?
                                } else {
                                    let code:ErrorCode=serde_json::from_value(serde_json::Value::String(record.failure.ok_or_else(||ApiError::internal("a completed creation has no outcome"))?)).map_err(ApiError::internal)?;
                                    let mut error=ApiError::new(code,"the first creating request was refused");error.problem.status=status.as_u16();return Err(error);
                                };
                                (Answer {body,status:Some(status),headers:idempotency::replay_headers()},false,true,reply_ctx)
                            }
                            pending => {
                                let pending = match pending {Some(Claim::New(pending))=>Some(pending),None=>None,_=>unreachable!()};
                                let capture = pending.as_ref().map(|pending|pending.capture.clone());
                                let resume_input = pending.as_ref()
                                    .filter(|pending|pending.capture.record.outcome.is_some() && creation.as_ref().is_some_and(|binding|binding.resume.is_some()))
                                    .map(|_|(query_text.clone(),body_bytes.clone()));
                                let finish_ctx = ctx.clone();
                                let public_retry = unassigned.as_ref().map(|_|(body_bytes.clone(),parts.uri.clone()));
                                let publication_context = &mut publication_ctx;
                                // The block keeps its own handle: the answer below still closes ended admissions with `app`.
                                let execution_app = app.clone();
                                let execution = async move {
                                    let app = execution_app;
                        if deferred_creation && operation.is_none() && ctx.user().is_ok_and(|user|matches!(&user.via,authentication::Via::Token{operation_scope:Some(_),..})) {
                            ctx = admit_implicit_open(ctx).await?;
                        }
                        if deferred_creation && (operation.is_none() || !ctx.user().is_ok_and(|user|matches!(&user.via,authentication::Via::Token{operation_scope:Some(_),..}))) {
                            ctx = authorize_open(ctx,query_text.as_deref()).await?;
                        }
                        if let Some(operation) = operation {
                            // Only a new attempt installs the writer. A completed replay
                            // owns its native Read projector and no spent writer state.
                            ctx.operation_request = Some(OperationRequest::new(route,operation,query_text.as_deref(),body_bytes.clone())?);
                            let rule_key = ctx.user().is_ok_and(|user| {
                                matches!(
                                    &user.via,
                                    authentication::Via::Token {
                                        operation_scope: Some(_),
                                        ..
                                    }
                                )
                            });
                            if !operation.snapshot_read() || initial.is_some() {
                                ctx = prepare_registered_operation::<E::Query, E::Body>(
                                    ctx,
                                    query_text.as_deref(),
                                    &body_bytes,
                                    initial,
                                )
                                .await?;
                                if rule_key
                                    && !operation.snapshot_read()
                                    && ctx.topic_creation_prefix().is_none()
                                {
                                    ctx = authorize_open(ctx, query_text.as_deref()).await?;
                                }
                            }
                        }
                        let reply_ctx = ctx.clone();
                        *publication_context = Some(reply_ctx.clone());
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
                        let retry_ctx = (!ctx.snapshot_read() && retried_after_refusal(&route))
                            .then(|| ctx.clone());
                        let ((answer, guarded), refused) = serve_once!(ctx, query, body);
                        let reply_ctx=reply_ctx.with_snapshot_inspection()?;
                        // The handler's own result is turned into the answer at once: only that is kept across a wait.
                        let answer: ApiResult<Answer<E::Reply>> = answer.map(Into::into);
                        let (answer, guarded) = if refused
                            && answer.is_err()
                            && let Some(mut ctx) = retry_ctx
                        {
                            // Refused at its commit because who may do what changed meanwhile: decided
                            // again, once, under the authority that now holds.
                            ctx.now = app.clock.now();
                            let query: E::Query = read_query(&route, query_text.as_deref())?;
                            let body: E::Body = decode_body(&route, &body_bytes)?;
                            if initial.is_some() {
                                // The preceding action has ended. Repeat only the existing safe
                                // initial refresh outside a transaction, then resolve anew.
                                ctx = prepare_registered_operation::<E::Query, E::Body>(
                                    ctx,
                                    query_text.as_deref(),
                                    &body_bytes,
                                    initial,
                                )
                                .await?;
                            }
                            let ((again, guarded), _) = serve_once!(ctx, query, body);
                            (again.map(Into::into), guarded)
                        } else {
                            (answer, guarded)
                        };
                        let answer: Answer<E::Reply> = answer?;
                                    Ok::<_,ApiError>((answer,guarded,reply_ctx))
                                };
                                let command_capture = idempotency::commands::capture(
                                    capture.as_ref(), creation.as_ref().and_then(|binding| binding.command));
                                let mut outcome = orch_db::idempotency::creation::scope_unassigned(unassigned,
                                    orch_db::idempotency::creation::scope(capture,
                                        idempotency::commands::scope(command_capture,Box::pin(execution)))).await;
                                let mut race_replayed = false;
                                if outcome.is_err() && let Some((query,bytes))=resume_input
                                    && let Some(pending)=pending.as_ref()
                                    && let Some((created,status))=pending.accepted(&finish_ctx).await? {
                                    let binding=creation.as_ref().ok_or_else(||ApiError::internal("a native continuation lost its binding"))?;
                                    let replay=binding.replay.ok_or_else(||ApiError::internal("a native continuation lost its projector"))?;
                                    outcome=replay(finish_ctx.clone(),read_query(&route,query.as_deref())?,decode_body(&route,&bytes)?,created).await
                                        .map(|body|(Answer {body,status:Some(status),headers:idempotency::replay_headers()},false,finish_ctx.clone()));
                                    race_replayed=outcome.is_ok();
                                    if race_replayed {crate::key_operations::activity::replayed();}
                                }
                                if matches!(&outcome,Err(error) if error.code()==ErrorCode::ConflictExists)
                                    && let Some((bytes,uri)) = public_retry {
                                    let binding=creation.as_ref().ok_or_else(||ApiError::internal("a public creation lost its binding"))?;
                                    let principal=binding.principal.ok_or_else(||ApiError::internal("a public creation lost its proof"))?;
                                    if let Some(account)=principal(finish_ctx.clone(),decode_body(&route,&bytes)?).await?
                                        && let Some((record,status))=creation_claims.find_proven(&finish_ctx,binding.kind,&uri,&bytes,account).await? {
                                        let created=record.outcome.ok_or_else(||ApiError::internal("a completed public creation lost its identity"))?;
                                        let replay=binding.replay.ok_or_else(||ApiError::internal("a public creation has no native replay"))?;
                                        let body=replay(finish_ctx.clone(),read_query(&route,uri.query())?,decode_body(&route,&bytes)?,created).await?;
                                        crate::key_operations::activity::replayed();
                                        outcome=Ok((Answer {body,status:Some(status),headers:idempotency::replay_headers()},false,finish_ctx.clone()));
                                        race_replayed=true;
                                    }
                                }
                                if let Some(pending) = pending {
                                    let (status,identity,failure) = match &outcome {
                                        Ok((answer,_,_)) => (answer.status.unwrap_or_else(||StatusCode::from_u16(route.status).unwrap_or(StatusCode::OK)),
                                            creation.as_ref().ok_or_else(||ApiError::internal("a creation has no native identity binding"))?.identity.map(|identity|identity(&answer.body)).transpose()?,None),
                                        Err(error) => (StatusCode::from_u16(error.problem.status).map_err(ApiError::internal)?,None,Some(error.code())),
                                    };
                                    pending.finish(&finish_ctx,status,identity,failure).await?;
                                }
                                let (answer,guarded,reply_ctx)=outcome?;(answer,guarded,race_replayed,reply_ctx)
                            }
                        };
                        publication_ctx = Some(reply_ctx.clone());
                        if operation.is_some_and(ConfiguredOperation::snapshot_read) && !guarded {
                            return Err(ApiError::internal(
                                "the snapshot reader did not check its current read guard",
                            ));
                        }
                        if let Some(prefix) = reply_ctx.topic_creation_prefix() {
                            prefix.require_complete()?;
                        }
                        if reply_ctx.key_receipt_required()
                            && !orch_api_contract::operations::for_route(&route)
                                .map_err(ApiError::internal)?
                                .receipt
                        {
                            return crate::key_operations::native::observed(&reply_ctx, "registry.content", Err(ApiError::new(
                                ErrorCode::AuthzDenied,
                                "the operation's response content has not been admitted",
                            )));
                        }
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
                            if !unguarded
                                && !replayed {transaction_guard::require_seen(&route,guarded)?;}
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
                    }))
                    .await;
                record_key_call(&app, &call, key_call_outcome(&outcome)).await;
                let outcome =
                    crate::key_operations::publication::result(publication_ctx, outcome).await;
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
        self.on_raw_configured(
            endpoint,
            None,
            RawInput::NoBodyRead,
            move |ctx, query, body, _raw_query| handler.clone()(ctx, query, body),
        )
    }

    /// A raw writer whose complete native footprint comes from its actual subject/query.
    /// Admission and ordinary authorization precede body consumption. The handler buffers its
    /// bounded bytes once and keeps its native row/file/first-effect guard after every wait.
    /// Completed results use a producer-supplied typed receipt/current response Read boundary;
    /// Registry never re-admits the already completed Write/Manage to publish that result.
    ///
    /// # Panics
    /// If the declaration is not a changing raw/byte-body operation.
    pub fn on_raw_query_with_optional_operation<E, F, Fut>(
        &mut self,
        endpoint: E,
        resolver: OptionalOperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        F: Fn(Ctx, E::Query, Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<Response>> + Send + 'static,
    {
        assert!(
            E::ROUTE.method != Method::GET && matches!(E::ROUTE.body, Shape::Bytes | Shape::Raw),
            "a raw changing query operation retains its actual raw body declaration"
        );
        self.on_raw_configured(
            endpoint,
            Some(ConfiguredOperation::Optional(resolver)),
            RawInput::ChangingQuery,
            move |ctx, query, body, _raw_query| handler.clone()(ctx, query, body),
        )
    }

    /// A body-selected native mutation: known destination/native gate, one bounded body,
    /// then complete current E preparation over the immutable native input. No body replay.
    ///
    /// # Panics
    /// If the declaration is not a changing raw/byte-body operation.
    pub fn on_raw_staged_operation<E, S, F, Fut>(&mut self, endpoint: E, handler: F) -> &mut Self
    where
        E: Endpoint,
        E::Query: Clone,
        S: crate::key_operations::native::raw::Staged<Query = E::Query>,
        F: Fn(Ctx, E::Query, Arc<S::Input>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<Response>> + Send + 'static,
    {
        assert!(
            E::ROUTE.method != Method::GET && matches!(E::ROUTE.body, Shape::Bytes | Shape::Raw),
            "a staged raw operation retains its actual raw body declaration"
        );
        self.on_raw_configured(
            endpoint,
            None,
            RawInput::ChangingBody,
            move |ctx, query: E::Query, body, raw_query| {
                let handler = handler.clone();
                async move {
                    let (ctx, input) = crate::key_operations::native::raw::take::<S>(
                        ctx,
                        query.clone(),
                        body,
                        raw_query.as_deref(),
                    )
                    .await?;
                    handler(ctx, query, input).await
                }
            },
        )
    }

    /// The same raw GET path with typed operation admission for its validated query.
    /// Such readers have no request-body operands. Their body stays owned by the existing
    /// handler, and current admission/declared permission is checked before any response leaves.
    ///
    /// # Panics
    /// If its declaration is not a GET with no request body.
    pub fn on_raw_read_with_operation<E, F, Fut>(
        &mut self,
        endpoint: E,
        resolver: OperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint<Body = ()>,
        F: Fn(Ctx, E::Query, Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<Response>> + Send + 'static,
    {
        assert!(
            E::ROUTE.method == Method::GET && matches!(E::ROUTE.body, Shape::None),
            "a raw read operation has no request-body operands"
        );
        self.on_raw_configured(
            endpoint,
            Some(ConfiguredOperation::Required(resolver)),
            RawInput::NoBodyRead,
            move |ctx, query, body, _raw_query| handler.clone()(ctx, query, body),
        )
    }

    /// A prepared raw native read resolves only inside its own watched snapshot, retaining
    /// the ordinary cold path. A modern scoped None still supplies no admission proof.
    ///
    /// # Panics
    /// If its declaration is not a GET with no request body.
    pub fn on_raw_read_with_optional_operation<E, F, Fut>(
        &mut self,
        endpoint: E,
        resolver: OptionalOperationResolver<E::Query, ()>,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint<Body = ()>,
        F: Fn(Ctx, E::Query, Body) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<Response>> + Send + 'static,
    {
        assert!(
            E::ROUTE.method == Method::GET && matches!(E::ROUTE.body, Shape::None),
            "a raw read operation has no request-body operands"
        );
        self.on_raw_configured(
            endpoint,
            Some(ConfiguredOperation::SnapshotOptional(resolver)),
            RawInput::NoBodyRead,
            move |ctx, query, body, _raw_query| handler.clone()(ctx, query, body),
        )
    }

    fn on_raw_configured<E, F, Fut>(
        &mut self,
        endpoint: E,
        operation: Option<ConfiguredOperation<E::Query, ()>>,
        input: RawInput,
        handler: F,
    ) -> &mut Self
    where
        E: Endpoint,
        F: Fn(Ctx, E::Query, Body, Option<String>) -> Fut + Clone + Send + Sync + 'static,
        Fut: Future<Output = ApiResult<Response>> + Send + 'static,
    {
        let _ = endpoint;
        let route = E::ROUTE;
        let unguarded = self.unguarded;
        let creations = self.creations.clone();
        let creation_claims = self.creation_claims.clone();
        let serve = move |State(app): State<App>, request: Request| {
            let handler = handler.clone();
            let creations = creations.clone();
            let creation_claims = creation_claims.clone();
            async move {
                let request_id = orch_types::new_id().to_string();
                let (mut parts, body) = request.into_parts();
                let call = activity::Call::http(
                    route,
                    request_id.clone(),
                    orch_types::Time::new(app.clock.now()),
                    remote(&app, &parts),
                );
                let mut publication_ctx = None;
                let outcome: ApiResult<Response> = call
                    .scope(async {
                        let creation=creations.get::<E::Query,E::Body,E::Reply>(route.name)?;
                        let deferred_creation=creation.as_ref().is_some_and(|binding|binding.raw.is_some()) && parts.headers.contains_key(orch_api_contract::common::IDEMPOTENCY_HEADER);
                        let mut ctx = Box::pin(open(
                            &app,
                            route,
                            &mut parts,
                            &request_id,
                            operation.is_some() || input == RawInput::ChangingBody,
                            deferred_creation,
                        ))
                        .await?;
                        publication_ctx = Some(ctx.clone());
                        if input == RawInput::ChangingBody
                            && parts.uri.query().is_some_and(|query| query.len() > MAX_JSON_BYTES)
                        {
                            return Err(ApiError::new(ErrorCode::RequestTooLarge, "the operation input is too large")
                                .with("limit", MAX_JSON_BYTES));
                        }
                        let query: E::Query = read_query(&route, parts.uri.query())?;
                        let mut untouched_body=Some(body);
                        let (claim,incoming)=if deferred_creation {
                            let _=idempotency::request_key(&ctx)?;
                            let binding=creation.as_ref().ok_or_else(||ApiError::internal("a raw creation has no native binding"))?;
                            let raw=binding.raw.as_ref().ok_or_else(||ApiError::internal("a raw creator has a JSON binding"))?;
                            let descriptor=(raw.input)(&ctx).await?;
                            let incoming=idempotency::RawIncoming::take(&ctx,untouched_body.take().ok_or_else(||ApiError::internal("the raw body was already consumed"))?,descriptor).await?;
                            let claim=creation_claims.begin(&ctx,binding.kind,&parts.uri,idempotency::BodyIdentity::Raw(&incoming)).await?;
                            (claim,Some(incoming))
                        } else {(None,None)};
                        let (response,guarded,replayed,reply_ctx)=match claim {
                            Some(Claim::Saved {account,record,status})=>{
                                crate::key_operations::activity::replayed();
                                if account!=ctx.user()?.id() {return Err(ApiError::internal("the completed raw record has another account"));}
                                let reply_ctx=ctx.clone();
                                let created=if let Some(created)=record.outcome {created}else {
                                    let code:ErrorCode=serde_json::from_value(serde_json::Value::String(record.failure.ok_or_else(||ApiError::internal("a completed raw creation has no outcome"))?)).map_err(ApiError::internal)?;
                                    let mut error=ApiError::new(code,"the first creating request was refused");error.problem.status=status.as_u16();return Err(error);
                                };
                                let binding=creation.as_ref().and_then(|binding|binding.raw.as_ref()).ok_or_else(||ApiError::internal("the completed raw creator has no native projector"))?;
                                let incoming=incoming.ok_or_else(||ApiError::internal("a raw creation lost its supplied input"))?;
                                let mut response=(binding.replay)(ctx,query,incoming,created).await?;
                                *response.status_mut()=status;
                                response.headers_mut().extend(idempotency::replay_headers());
                                (Ok(response),false,true,reply_ctx)
                            }
                            pending=>{
                                let pending=match pending {Some(Claim::New(pending))=>Some(pending),None=>None,_=>unreachable!()};
                                let capture=pending.as_ref().map(|pending|pending.capture.clone());
                                let finish_ctx=ctx.clone();
                                let publication_context=&mut publication_ctx;
                                let body=if let Some(incoming)=incoming {incoming.into_body().await?}else{untouched_body.take().ok_or_else(||ApiError::internal("the raw body was already consumed"))?};
                                let execution=async move {
                                    let modern=ctx.user().is_ok_and(|user|matches!(user.via,authentication::Via::Token {operation_scope:Some(_),..}));
                                    if deferred_creation && operation.is_none() && input!=RawInput::ChangingBody && modern {ctx=admit_implicit_open(ctx).await?;}
                                    if deferred_creation && (!modern || (operation.is_none() && input!=RawInput::ChangingBody)) {ctx=authorize_open(ctx,parts.uri.query()).await?;}
                        if let Some(operation) = operation {
                            ctx.operation_request = Some(match input {
                                RawInput::ChangingQuery => {
                                    let ConfiguredOperation::Optional(resolver) = operation else {
                                        return Err(ApiError::internal(
                                            "the raw query writer changed its input kind",
                                        ));
                                    };
                                    OperationRequest::raw_query(route, resolver, parts.uri.query())?
                                }
                                RawInput::NoBodyRead => OperationRequest::new(
                                    route,
                                    operation,
                                    parts.uri.query(),
                                    bytes::Bytes::new(),
                                )?,
                                RawInput::ChangingBody => return Err(ApiError::internal(
                                    "a staged raw operation cannot decode its body during entry preparation",
                                )),
                            });
                            let rule_key = ctx.user().is_ok_and(|user| {
                                matches!(
                                    &user.via,
                                    authentication::Via::Token {
                                        operation_scope: Some(_),
                                        ..
                                    }
                                )
                            });
                            if !operation.snapshot_read() {
                                ctx = prepare_registered_operation::<E::Query, ()>(
                                    ctx,
                                    parts.uri.query(),
                                    &[],
                                    None,
                                )
                                .await?;
                                if rule_key {
                                    ctx = authorize_open(ctx, parts.uri.query()).await?;
                                }
                            }
                        }
                        let reply_ctx = ctx.clone();
                        *publication_context = Some(reply_ctx.clone());
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
                                            Box::pin(handler(ctx, query, body, parts.uri.query().map(str::to_owned))),
                                        ),
                                    ),
                                ),
                            )),
                        )
                        .await;
                                Ok::<_,ApiError>((response,guarded,reply_ctx))
                                };
                                let outcome=orch_db::idempotency::creation::scope(capture,Box::pin(execution)).await;
                                if let Some(pending)=pending {
                                    let (status,failure)=match &outcome {
                                        Ok((Ok(response),_,_))=>(response.status(),(!response.status().is_success()).then_some(ErrorCode::Internal)),
                                        Ok((Err(error),_,_))|Err(error)=>(StatusCode::from_u16(error.problem.status).map_err(ApiError::internal)?,Some(error.code())),
                                    };
                                    pending.finish(&finish_ctx,status,None,failure).await?;
                                }
                                let (response,guarded,reply_ctx)=outcome?;
                                (response,guarded,false,reply_ctx)
                            }
                        };
                        publication_ctx=Some(reply_ctx.clone());
                        #[cfg(any(test, feature = "test-support"))]
                        crate::realtime::history_read_tests::pause(
                            crate::realtime::history_read_tests::Cut::RawContentReturn,
                        )
                        .await;
                        let reply_ctx=reply_ctx.with_snapshot_inspection()?;
                        let snapshot = operation.is_some_and(ConfiguredOperation::snapshot_read);
                        if !replayed && snapshot && response.is_ok() && !guarded {
                            return Err(ApiError::internal(
                                "the raw snapshot reader did not check its current read guard",
                            ));
                        }
                        if !replayed && operation.is_some() && !snapshot && input == RawInput::NoBodyRead {
                            // The existing response owns any opened immutable bytes/stream. Resolve
                            // this same operation and current credential again before returning it,
                            // including an error response, without inspecting or buffering its body.
                            let mut conn = app.db.acquire().await?;
                            let principal = reply_ctx.operation_principal(&mut conn).await?;
                            if let Access::Permission(permission) = route.access {
                                authorization::require_on_path_in(
                                    &reply_ctx, &mut conn, &principal, permission,
                                )
                                .await?;
                            }
                            if reply_ctx.key_receipt_required() {
                                return Err(ApiError::new(
                                    ErrorCode::AuthzDenied,
                                    "the operation's response content has not been admitted",
                                ));
                            }
                        }
                        if snapshot && response.is_ok() && reply_ctx.key_receipt_required() {
                            return Err(ApiError::new(
                                ErrorCode::AuthzDenied,
                                "the raw response content has not been admitted",
                            ));
                        }
                        if matches!(input, RawInput::ChangingQuery | RawInput::ChangingBody)
                            && response.is_ok()
                            && reply_ctx.key_receipt_required()
                            && !orch_api_contract::operations::for_route(&route)
                                .map_err(ApiError::internal)?
                                .receipt
                        {
                            return Err(ApiError::new(
                                ErrorCode::AuthzDenied,
                                "the raw operation's response content has not been admitted",
                            ));
                        }
                        let mut response = response?;
                        if response.status().is_success() {
                            Box::pin(crate::middleware::admin_openings::finish_raw(&reply_ctx))
                                .await?;
                            if !unguarded && !replayed {
                                transaction_guard::require_seen(&route, guarded)?;
                            }
                            if may_end_admission(&route) {
                                // As for a JSON request: what this change ended is closed first.
                                Box::pin(crate::ports_runtime::close_ended(&app)).await;
                            }
                            // Keep the current-authority inspection guard outside the raw
                            // handler's inline future, as for the other post-response guards.
                            Box::pin(crate::middleware::inspection::finish(&reply_ctx)).await?;
                            crate::middleware::inspection::headers(&mut response, &reply_ctx)?;
                        }
                        Ok(response)
                    })
                    .await;
                record_key_call(&app, &call, key_call_outcome(&outcome)).await;
                let outcome =
                    crate::key_operations::publication::result(publication_ctx, outcome).await;
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
                let call = activity::Call::http(
                    route,
                    request_id.clone(),
                    orch_types::Time::new(app.clock.now()),
                    remote(&app, &parts),
                );
                let outcome: ApiResult<Response> = call
                    .scope(async {
                        let ctx =
                            Box::pin(open(&app, route, &mut parts, &request_id, false, false))
                                .await?;
                        let query: E::Query = read_query(&route, parts.uri.query())?;
                        let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &app)
                            .await
                            .map_err(|rejection| {
                                ApiError::new(ErrorCode::RequestInvalid, rejection.to_string())
                            })?;
                        Ok(handler(ctx, query, upgrade).await)
                    })
                    .await;
                record_key_call(&app, &call, key_call_outcome(&outcome)).await;
                match outcome {
                    Ok(response) => finish(response, &request_id),
                    Err(error) => error.into_response_for(&request_id),
                }
            }
        };
        self.add(route, on(filter(route.method), serve))
    }

    fn add(&mut self, route: Route, method_router: axum::routing::MethodRouter<App>) -> &mut Self {
        if HttpSurface::Browser.serves(&route) {
            let router = std::mem::take(&mut self.router);
            self.router = router.route(route.path, method_router.clone());
        }
        if HttpSurface::Key.serves(&route) {
            let router = std::mem::take(&mut self.key_router);
            self.key_router = router.route(route.path, method_router);
        }
        self.bound.push(route);
        self
    }

    /// The requests that have a handler.
    #[must_use]
    pub fn bound(&self) -> &[Route] {
        &self.bound
    }

    /// Existing browser/public router, including the internal downloads-only subtree.
    /// Its caller must mount it under the browser/public path; account keys are never accepted.
    pub fn into_router(self) -> axum::Router<App> {
        self.into_router_for(HttpSurface::Browser)
    }

    /// One explicitly selected surface, for limited protocol subtrees such as internal downloads.
    pub fn into_router_for(self, surface: HttpSurface) -> axum::Router<App> {
        let router = match surface {
            HttpSurface::Browser => self.router,
            HttpSurface::Key => self.key_router,
        };
        Self::mounted(router, surface)
    }

    /// In-process owner reads reuse the same business handlers without fabricating an HTTP plane.
    /// open() accepts this router only with the private checked native read extension.
    pub fn into_native_read_router(self) -> axum::Router<App> {
        self.router.fallback(|| async {
            ApiError::not_found().into_response_for(&orch_types::new_id().to_string())
        })
    }

    /// Both HTTP surfaces reuse exactly the same registered handler entries.
    pub fn into_surface_routers(self) -> (axum::Router<App>, axum::Router<App>) {
        (
            Self::mounted(self.router, HttpSurface::Browser),
            Self::mounted(self.key_router, HttpSurface::Key),
        )
    }

    fn mounted(router: axum::Router<App>, surface: HttpSurface) -> axum::Router<App> {
        router.layer(axum::Extension(surface)).fallback(|| async {
            ApiError::not_found().into_response_for(&orch_types::new_id().to_string())
        })
    }
}

#[cfg(any(test, feature = "test-support"))]
#[path = "registry/action_guard_state_tests.rs"]
pub mod action_guard_state_tests;

#[cfg(any(test, feature = "test-support"))]
#[path = "registry/query_tests.rs"]
pub mod query_tests;
