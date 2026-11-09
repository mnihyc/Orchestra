//! Restrict the same caller using all effects resolved by the ordinary producer.
//!
//! A producer selects its variant from typed inputs and stored facts, fills every
//! declared facet on its current guarded connection, then enters ordinary authorization
//! with the returned caller. Neither operation names nor target ancestry come from a key.

use super::resolver::{Disclosure, OperationData};
use super::{activity, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use futures::future::BoxFuture;
use orch_access_contract::key_scope::{KeyAction, KeyResolvedTarget, KeyTargetKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::operations::{self, Delegation, OperationVariant, TargetResolver};
use orch_api_contract::routes::Route;
use orch_authz::Caller;
use orch_db::Conn;
use orch_engine::operation_scope::{self, BindingError, compatible_selection};
use std::borrow::Cow;

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this operation",
    )
}

pub(crate) fn binding_error(error: BindingError) -> ApiError {
    match error {
        BindingError::Catalogue(error) => ApiError::internal(error),
        BindingError::Refused => refused(),
    }
}

/// Server-owned, request-local facts; never deserialized or retained for deferred work.
/// Clones are only for alternative effect-free preparations at the same checkpoint.
#[derive(Clone)]
pub struct Prepared {
    core: operation_scope::Prepared,
    data: OperationData,
    topic_prefix: bool,
    restriction: Option<Caller>,
}

/// Scope admission does not grant resource authority. The original caller still
/// goes through the existing authorization and business operation.
pub(crate) struct Admission {
    pub caller: Caller,
    /// Existing-content response facets are separately allowed; otherwise the
    /// operation returns its receipt through the shared response projection.
    pub content: bool,
    pub disclosure: Disclosure,
    pub data: OperationData,
}

#[derive(Clone)]
enum Projection {
    CompletedResponse,
    CompletedContent,
    CompletedReplay(Vec<usize>),
    CompletedReplayReads {
        content: Vec<usize>,
        required_reads: Vec<usize>,
    },
    RequiredOperation,
    Refusal(Vec<usize>),
}

fn response_selection_matches(current: &Caller, selection: &str, response_only: bool) -> bool {
    current.grant().is_none()
        || current.admitted_operation() == Some(selection)
        || response_only && current.admitted_operation().is_none()
}

/// Purpose of an optional native response; completed replay never re-admits spent inputs.
#[derive(Clone)]
pub(crate) enum ResponseProjection {
    /// A completed native outcome: only actual current output/source-of-output Read.
    CompletedContent,
    Refusal(Vec<usize>),
    Replay(Vec<usize>),
    /// Server-declared native name Read roles, independently of any spent effect inputs.
    ReplayReads {
        content: Vec<usize>,
        required_reads: Vec<usize>,
    },
}

#[cfg(test)]
mod origin_tests;

impl Prepared {
    /// `selection` is the exact server-derived ID, including a compound suffix.
    pub fn new(route: Route, selection: &str) -> ApiResult<Self> {
        Ok(Self {
            core: operation_scope::Prepared::new(route, selection).map_err(binding_error)?,
            data: OperationData::default(),
            topic_prefix: false,
            restriction: None,
        })
    }

    /// The compound topic creator may defer dependent effects until its new Topic exists.
    /// This marker never changes `admit`: a partial plan is still refused by that method.
    pub(crate) fn topic_creation_prefix(mut self) -> ApiResult<Self> {
        if self.variant().operation != "TopicCreate.with_thread" {
            return Err(refused());
        }
        self.topic_prefix = true;
        Ok(self)
    }

    /// The existing-topic creator may also need rollbackable Environment/Thread identities
    /// before its dependent effects can be bound. The full gate remains unchanged.
    pub(crate) fn thread_creation_prefix(mut self) -> ApiResult<Self> {
        if self.variant().operation != "ThreadCreate" {
            return Err(refused());
        }
        self.topic_prefix = true;
        Ok(self)
    }

    /// Native adoption needs its actual new Thread before its gift sources can be bound.
    /// This closed marker preserves the initial plan's original/current restrictions.
    pub(crate) fn adoption_creation_prefix(mut self) -> ApiResult<Self> {
        if self.variant().operation != "ThreadAdopt" {
            return Err(refused());
        }
        self.topic_prefix = true;
        Ok(self)
    }

    /// Credential bounds only, for the opaque creator stage. This supplies no target
    /// admission and must survive replacing the known plan by its private root checkpoint.
    pub(crate) fn creation_prefix_restrictions(&self, caller: &Caller) -> ApiResult<Caller> {
        if !self.topic_prefix {
            return Err(refused());
        }
        Ok(self.narrowed_caller(caller)?.into_owned())
    }

    pub(crate) fn is_topic_creation_prefix(&self) -> bool {
        self.topic_prefix
    }

    /// Wraps the complete required creator producer returned by the same Engine writer.
    pub(crate) fn from_shared(core: operation_scope::Prepared) -> Self {
        Self {
            core,
            data: OperationData::default(),
            topic_prefix: false,
            restriction: None,
        }
    }

    pub fn variant(&self) -> &OperationVariant {
        self.core.variant()
    }

    /// Shared producer access to this exact preparation; HTTP data and disclosure stay here.
    pub(crate) fn shared(&self) -> &operation_scope::Prepared {
        &self.core
    }

    pub(crate) fn shared_mut(&mut self) -> &mut operation_scope::Prepared {
        &mut self.core
    }

    pub(crate) fn matches_route(&self, route: &Route) -> bool {
        self.core.matches_route(route)
    }

    /// Intersect a native continuation's accepted operation with this actual HTTP
    /// caller. Restoration supplies no reusable target admission, and a different
    /// selection/account cannot become a ceiling for this plan.
    pub(crate) async fn with_origin(
        mut self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
        origin: &orch_authz::CapturedOperation,
    ) -> ApiResult<Self> {
        if origin.operation() != self.variant().operation
            || origin.user() != caller.user()
            || !authorization::same_credential_origin(ctx.user()?, caller)
        {
            return Err(refused());
        }
        let restored = origin
            .restore_current(conn, ctx.app.clock.as_ref())
            .await
            .map_err(authorization::restoration_error)?
            .ok_or_else(refused)?;
        let caller = self.narrowed_caller(caller)?;
        let restricted = caller
            .intersect_restrictions(&restored)
            .map_err(|_| refused())?;
        if !restricted.key_deadline_allows(ctx.app.clock.as_ref()) {
            return Err(refused());
        }
        ctx.publication.retain_deadline(
            restricted
                .grant()
                .and_then(orch_authz::TokenGrant::expires_at),
        )?;
        self.restriction = Some(restricted);
        Ok(self)
    }

    fn narrowed_caller<'a>(&self, caller: &'a Caller) -> ApiResult<Cow<'a, Caller>> {
        match &self.restriction {
            Some(restriction) => caller
                .intersect_restrictions(restriction)
                .map(Cow::Owned)
                .map_err(|_| refused()),
            None => Ok(Cow::Borrowed(caller)),
        }
    }

    /// Retain an already restored request-input ceiling without turning its partial known
    /// facts into target admission. The complete producer still must admit every required role.
    pub(crate) fn within_caller(mut self, ctx: &Ctx, caller: &Caller) -> ApiResult<Self> {
        if !self.matches_route(&ctx.route)
            || !same_response_origin(ctx, caller)?
            || !caller.key_deadline_allows(ctx.app.clock.as_ref())
        {
            return Err(refused());
        }
        self.core.compatible(caller).map_err(binding_error)?;
        let restricted = self.narrowed_caller(caller)?.into_owned();
        ctx.publication.retain_deadline(
            restricted
                .grant()
                .and_then(orch_authz::TokenGrant::expires_at),
        )?;
        self.restriction = Some(restricted);
        Ok(self)
    }

    /// Retain effect operands captured by this exact preparation, without serializing or
    /// logging them. A fresh guarded preparation replaces them, including on retries.
    #[must_use]
    pub fn with_data<T: Send + Sync + 'static>(mut self, data: T) -> Self {
        self.data = OperationData::new(data);
        self
    }

    /// The actual business kind is independent of a creation destination's kind.
    /// An empty or duplicate binding never silently drops a required effect.
    pub fn bind(
        &mut self,
        index: usize,
        resource_kind: KeyTargetKind,
        resolved: &[KeyResolvedTarget],
    ) -> ApiResult<()> {
        self.core
            .bind(index, resource_kind, resolved)
            .map_err(binding_error)
    }

    /// A typed collection producer explicitly found no effects/disclosures. This cannot
    /// fill an unclassified or single-object role, and a missing binding still fails.
    pub fn bind_empty(&mut self, index: usize) -> ApiResult<()> {
        self.core.bind_empty(index).map_err(binding_error)
    }

    /// One actual collection may contain several stored business families. The shared
    /// binder validates each explicit family without inferring it from the target kind.
    pub fn bind_collection(
        &mut self,
        index: usize,
        groups: &[(KeyTargetKind, Vec<KeyResolvedTarget>)],
    ) -> ApiResult<()> {
        self.core
            .bind_collection(index, groups)
            .map_err(binding_error)
    }

    /// Scope-only check for a proposed source/row under this same exact operation.
    /// The ordinary producer still checks account visibility before materializing it.
    pub fn permits_binding(
        &self,
        caller: &Caller,
        index: usize,
        kind: KeyTargetKind,
        targets: &[KeyResolvedTarget],
    ) -> ApiResult<bool> {
        let caller = self.narrowed_caller(caller)?;
        self.core
            .permits_binding(&caller, index, kind, targets)
            .map_err(binding_error)
    }

    /// Admit known destination/proposed source facets for effect-free preparation of the
    /// actual source set. No missing facet becomes allowed: final `admit` still requires
    /// the complete footprint before effects. The same account and credential are retained.
    pub async fn preparation_principal(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
    ) -> ApiResult<orch_authz::Principal> {
        let caller = self.narrowed_caller(caller)?;
        self.core.compatible(&caller).map_err(binding_error)?;
        if !self.matches_route(&ctx.route)
            || !authorization::same_credential_origin(ctx.user()?, &caller)
        {
            return Err(refused());
        }
        // Typed error builders see the same conservative disclosure boundary before
        // ordinary validation can construct a named error or existing-content conflict.
        ctx.checkpoint_disclosure(self.disclosure(&caller));
        let caller = self
            .core
            .preparation_caller(&caller)
            .map_err(binding_error)?;
        let principal =
            authorization::load_principal(conn, caller, orch_types::Time::new(ctx.app.clock.now()))
                .await?;
        crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await
    }

    /// A complete declared prerequisite row, with no exact operation or effect proof.
    /// The shared effect entry binds the row before this ordinary authorization step.
    pub(crate) async fn prerequisite_principal(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
    ) -> ApiResult<orch_authz::Principal> {
        let caller = self.narrowed_caller(caller)?;
        self.core.compatible(&caller).map_err(binding_error)?;
        if !self.matches_route(&ctx.route)
            || !authorization::same_credential_origin(ctx.user()?, &caller)
        {
            return Err(refused());
        }
        ctx.checkpoint_disclosure(self.disclosure(&caller));
        let known = self.core.known_required_facets();
        let caller = if known.is_empty() {
            // This supplies no checked-empty proof, including when a genuine known
            // collection is empty and a later creator source is still unresolved.
            caller.prepare_unresolved()
        } else {
            caller
                .admit_native_preparation(&known)
                .map_err(|_| refused())?
        };
        let principal = authorization::load_principal(conn, caller, ctx.app.clock.time()).await?;
        crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await
    }

    fn disclosure(&self, caller: &Caller) -> Disclosure {
        match self.core.response_coverage(caller) {
            Some(true) => Disclosure::Full,
            Some(false) => Disclosure::Denied,
            None => Disclosure::Pending,
        }
    }

    pub(crate) fn admit(self, caller: &Caller) -> ApiResult<Admission> {
        let caller = self.narrowed_caller(caller)?;
        self.core.compatible(&caller).map_err(binding_error)?;
        let disclosure = self.disclosure(&caller);
        let required = self.core.required_facets().map_err(binding_error)?;
        activity::selected(&self.core.variant().operation, &required);
        if required.is_empty() {
            // Preserve the ordinary gate's scope-refusal precedence for a checked-empty act.
            self.core.admit(&caller, false).map_err(binding_error)?;
        }
        // An effectful producer must already have a typed receipt branch before a key
        // can perform its write without guaranteed content. Read projections can finish
        // their pending row set before serialization without committing business effects.
        if !self.core.receipt()
            && (disclosure == Disclosure::Denied
                || (disclosure == Disclosure::Pending
                    && required.iter().any(|facet| facet.action != KeyAction::Read)))
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the operation needs its typed receipt projection",
            ));
        }
        let admitted = self
            .core
            .admit(&caller, disclosure == Disclosure::Full)
            .map_err(binding_error)?;
        Ok(Admission {
            caller: admitted,
            content: disclosure == Disclosure::Full,
            disclosure,
            data: self.data,
        })
    }

    /// Complete optional content only after its actual targets exist. The callback is
    /// the ordinary Read/projection producer and receives the same freshly restored,
    /// accepted/current-intersected caller. No callback runs for a scope refusal.
    /// Required effects were already admitted before mutation; this cannot add effects.
    pub fn read_response<'a, T, F>(
        self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        current: &'a Caller,
        read: F,
    ) -> BoxFuture<'a, ApiResult<Option<T>>>
    where
        T: 'a,
        F: for<'read> FnOnce(
                &'read mut Conn,
                &'read orch_authz::Principal,
            ) -> BoxFuture<'read, ApiResult<T>>
            + Send
            + 'a,
    {
        Box::pin(self.project(ctx, conn, current, Projection::CompletedResponse, read))
    }

    /// A typed failed native act projects only its actual declared optional Read sources.
    /// It admits no unperformed required Write/Manage or absent success response subject.
    pub(crate) fn read_refusal<'a, T, F>(
        self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        current: &'a Caller,
        indices: Vec<usize>,
        read: F,
    ) -> BoxFuture<'a, ApiResult<Option<T>>>
    where
        T: 'a,
        F: for<'read> FnOnce(
                &'read mut Conn,
                &'read orch_authz::Principal,
            ) -> BoxFuture<'read, ApiResult<T>>
            + Send
            + 'a,
    {
        Box::pin(self.project(ctx, conn, current, Projection::Refusal(indices), read))
    }

    /// The same projector with an explicit completed-replay response-only purpose.
    pub(crate) fn read_response_projection<'a, T, F>(
        self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        current: &'a Caller,
        projection: ResponseProjection,
        read: F,
    ) -> BoxFuture<'a, ApiResult<Option<T>>>
    where
        T: 'a,
        F: for<'read> FnOnce(
                &'read mut Conn,
                &'read orch_authz::Principal,
            ) -> BoxFuture<'read, ApiResult<T>>
            + Send
            + 'a,
    {
        match projection {
            ResponseProjection::CompletedContent => {
                Box::pin(self.project(ctx, conn, current, Projection::CompletedContent, read))
            }
            ResponseProjection::Refusal(indices) => {
                self.read_refusal(ctx, conn, current, indices, read)
            }
            ResponseProjection::Replay(indices) => Box::pin(self.project(
                ctx,
                conn,
                current,
                Projection::CompletedReplay(indices),
                read,
            )),
            ResponseProjection::ReplayReads {
                content,
                required_reads,
            } => Box::pin(self.project(
                ctx,
                conn,
                current,
                Projection::CompletedReplayReads {
                    content,
                    required_reads,
                },
                read,
            )),
        }
    }

    /// A native read keeps its complete current required operation, including a
    /// real Manage inspection or explicitly registered effect-free body preview.
    /// An explicitly registered pure snapshot may require native Write/Manage prerequisites
    /// without executing them. Completed mutation readback uses read_response instead.
    pub fn read_operation<'a, T, F>(
        self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        current: &'a Caller,
        read: F,
    ) -> BoxFuture<'a, ApiResult<Option<T>>>
    where
        T: 'a,
        F: for<'read> FnOnce(
                &'read mut Conn,
                &'read orch_authz::Principal,
            ) -> BoxFuture<'read, ApiResult<T>>
            + Send
            + 'a,
    {
        Box::pin(async move {
            // The explicit snapshot registration owns a read-only transaction. Its
            // canonical prerequisites can include native Use expressed as Write;
            // they stay fully admitted here without becoming a mutation boundary.
            if !ctx.snapshot_read()
                && (ctx.route.method != orch_api_contract::routes::Method::GET
                    || self
                        .core
                        .required_facets()
                        .map_err(binding_error)?
                        .iter()
                        .any(|facet| facet.action == KeyAction::Write))
            {
                return Err(ApiError::internal(
                    "a changing operation needs its effect boundary",
                ));
            }
            let work: BoxFuture<'a, ApiResult<Option<T>>> =
                Box::pin(self.project(ctx, conn, current, Projection::RequiredOperation, read));
            work.await
        })
    }

    async fn project<T, F>(
        self,
        ctx: &Ctx,
        conn: &mut Conn,
        current: &Caller,
        projection: Projection,
        read: F,
    ) -> ApiResult<Option<T>>
    where
        F: for<'a> FnOnce(
            &'a mut Conn,
            &'a orch_authz::Principal,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>,
    {
        super::native::entering(ctx, "response.project");
        super::native::observed(
            ctx,
            "response.plan",
            crate::ask::tools::check_plan(ctx, conn, &self).await,
        )?;
        if !self.matches_route(&ctx.route) {
            return Err(refused());
        }
        if ctx.disclosure() == Disclosure::Denied {
            return super::native::observed(ctx, "response.disclosure", Ok(None));
        }
        let response_only = matches!(
            projection,
            Projection::CompletedContent
                | Projection::CompletedReplay(_)
                | Projection::CompletedReplayReads { .. }
        );
        if !response_selection_matches(current, &self.core.variant().operation, response_only) {
            return Err(refused());
        }
        let content = match &projection {
            Projection::Refusal(indices) | Projection::CompletedReplay(indices) => {
                self.core.response_facets_for(indices)
            }
            Projection::CompletedReplayReads {
                content,
                required_reads,
            } => self.core.response_read_facets_for(content, required_reads),
            _ => self.core.response_facets(),
        }
        .map_err(binding_error)?;
        let required = match &projection {
            Projection::CompletedResponse => self.core.required_reads(),
            Projection::RequiredOperation => self.core.required_facets(),
            Projection::CompletedContent
            | Projection::Refusal(_)
            | Projection::CompletedReplay(_)
            | Projection::CompletedReplayReads { .. } => Ok(Vec::new()),
        }
        .map_err(binding_error)?;
        let now = orch_types::Time::new(ctx.app.clock.now());
        let Some(identity) = response_caller_for_projection(
            ctx,
            conn,
            current,
            &self.core.variant().operation,
            now,
            response_only,
        )
        .await?
        else {
            return Ok(None);
        };
        let caller = self.narrowed_caller(&identity.caller)?.into_owned();
        if (!content.is_empty() && !caller.permits_facets(&content))
            || !compatible_selection(&caller, self.core.variant())
        {
            ctx.complete_disclosure(Disclosure::Denied);
            return Ok(None);
        }
        // Never reuse the effect checkpoint's old ancestry. A read root is supplied
        // freshly; a receipt with no content has no need for public Read admission.
        let mut facts = required;
        facts.extend(content);
        let mut recorded = current.admitted_facets().unwrap_or(&[]).to_vec();
        recorded.extend(facts.iter().cloned());
        let caller = if matches!(projection, Projection::RequiredOperation) {
            match self.core.admit(&caller, true) {
                Ok(admitted) => admitted,
                Err(_) => {
                    ctx.complete_disclosure(Disclosure::Denied);
                    return Ok(None);
                }
            }
        } else if facts.is_empty() {
            caller
        } else {
            match caller.admit_facets(&facts) {
                Ok(admitted) => admitted.into_caller(),
                Err(_) => {
                    ctx.complete_disclosure(Disclosure::Denied);
                    return Ok(None);
                }
            }
        };
        let deadline_caller = caller.clone();
        let result = async {
            let principal = authorization::load_principal(conn, caller, identity.now).await?;
            let principal =
                crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
            read(conn, &principal).await
        }
        .await;
        // Projection can wait after the current HTTP identity was restored. Keep
        // both successful content and named errors private once that exact
        // credential (or any accepted key ceiling) has reached its deadline.
        if identity
            .expires_at
            .is_some_and(|end| end <= ctx.app.clock.time())
            || !deadline_caller.key_deadline_allows(ctx.app.clock.as_ref())
        {
            ctx.complete_disclosure(Disclosure::Denied);
            return Ok(None);
        }
        let result = super::native::observed(ctx, "response.native", result);
        match result {
            Ok(value) => {
                ctx.complete_disclosure(Disclosure::Full);
                if ctx.disclosure() == Disclosure::Full {
                    activity::selected(&self.core.variant().operation, &recorded);
                }
                Ok((ctx.disclosure() == Disclosure::Full).then_some(value))
            }
            Err(error) => {
                ctx.complete_disclosure(Disclosure::Denied);
                Err(error)
            }
        }
    }
}

/// Exact transport credential origin, allowing only a narrowing of an ordinary session mode.
pub(crate) fn same_response_origin(ctx: &Ctx, current: &Caller) -> ApiResult<bool> {
    let original = ctx.authenticated_user()?;
    // The same Session may already have used a narrowed ordinary mode at its
    // current writer checkpoint. A different account/key or elevated mode is
    // never a substitute for the original authenticated HTTP credential.
    let same_origin = match (&original.via, current.grant()) {
        (crate::middleware::authentication::Via::Session { .. }, None) => {
            current.user() == original.id()
                && current.owner_read_source().is_none()
                && (!current.as_admin() || original.as_admin)
        }
        _ => authorization::same_credential_origin(original, current),
    };
    Ok(same_origin)
}

/// The existing response identity boundary, reusable before selecting optional source names.
/// Callers retain their existing disclosure/route checks; the pure origin/selection checks here
/// also keep this helper safe for current response preparation outside read_response.
pub(crate) async fn response_caller(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Caller,
    selection: &str,
    now: orch_types::Time,
) -> ApiResult<Option<crate::middleware::transaction_guard::CurrentHttpIdentity>> {
    response_caller_for_projection(ctx, conn, current, selection, now, false).await
}

/// Only the explicit response-only projector may supply a current unprepared credential.
/// Its ordinary/current response Read admission still follows this exact identity restoration.
async fn response_caller_for_projection(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Caller,
    selection: &str,
    _now: orch_types::Time,
    response_only: bool,
) -> ApiResult<Option<crate::middleware::transaction_guard::CurrentHttpIdentity>> {
    let same_origin = same_response_origin(ctx, current)?;
    if !same_origin || !response_selection_matches(current, selection, response_only) {
        return Err(refused());
    }
    // A producer may retain additional accepted key ceilings beyond the HTTP
    // credential. Preserve their existing current validity too; this never adds
    // a Session id/deadline to the native CapturedCaller model.
    let accepted = if current.grant().is_some() {
        match orch_authz::CapturedCaller::capture(current)
            .restore_current(conn, ctx.app.clock.as_ref())
            .await
            .map_err(authorization::restoration_error)?
        {
            Some(caller) => caller,
            None => {
                ctx.complete_disclosure(Disclosure::Denied);
                return Ok(None);
            }
        }
    } else {
        current.clone()
    };
    let mut identity = match crate::middleware::transaction_guard::current_http_identity(
        ctx,
        conn,
        crate::middleware::transaction_guard::HttpIdentityMode::Read,
    )
    .await
    {
        Ok(identity) => identity,
        Err(error)
            if matches!(
                error.code(),
                ErrorCode::AuthInvalidToken | ErrorCode::AuthRequired
            ) =>
        {
            ctx.complete_disclosure(Disclosure::Denied);
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    // A frozen callback can return data prepared under the supplied native
    // admin context. It cannot borrow a newly narrowed context to publish that
    // old body. Future fresh requests may still use their current ordinary rights.
    if current.as_admin() && !identity.caller.as_admin() {
        ctx.complete_disclosure(Disclosure::Denied);
        return Ok(None);
    }
    identity.caller = identity
        .caller
        .intersect_restrictions(&accepted)
        .map_err(ApiError::internal)?;
    if !identity.caller.key_deadline_allows(ctx.app.clock.as_ref()) {
        ctx.complete_disclosure(Disclosure::Denied);
        return Ok(None);
    }
    identity.user.as_admin = identity.caller.as_admin();
    ctx.publication.retain_deadline(identity.expires_at)?;
    ctx.publication.retain_deadline(
        identity
            .caller
            .grant()
            .and_then(orch_authz::TokenGrant::expires_at),
    )?;
    Ok(Some(identity))
}

/// Empty proof is accepted only for this exact catalogue selection and its declared
/// required collections, never for unresolved/singleton fields or another request.
pub(crate) fn checked_empty_for(route: &Route, caller: &Caller) -> ApiResult<bool> {
    operation_scope::checked_empty_for(route, caller).map_err(binding_error)
}

/// Install current admission without replacing the acting account/credential. All clones
/// of this request retain a later narrowing of its response, including commit retries.
pub(crate) fn install(ctx: Ctx, admission: Admission) -> ApiResult<Ctx> {
    debug_assert_eq!(admission.content, admission.disclosure == Disclosure::Full);
    ctx.publish_operation_data(admission.data)?;
    ctx.checkpoint_disclosure(admission.disclosure);
    let ctx = ctx.with_admitted_caller(admission.caller)?;
    Ok(ctx)
}

/// Common admission for declarations whose complete effects are already resolved by
/// the ordinary permission subject/current-account/system producer. Compound/body-selected
/// operations deliberately return None: their existing typed producer must supply the facts.
/// This helper must be called again on the mutation transaction's fresh connection.
pub(crate) async fn at_path(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<Option<Admission>> {
    if caller
        .grant()
        .and_then(|grant| grant.operation_scope())
        .is_none()
        && ctx.ask_read.is_none()
    {
        return Ok(None);
    }
    match prepare_at_path(ctx, conn).await? {
        Some(prepared) => {
            crate::ask::tools::check_plan(ctx, conn, &prepared).await?;
            prepared.admit(caller).map(Some)
        }
        None => Ok(None),
    }
}

/// Prepare the same complete factual path plan for a native typed producer.
/// Ordinary callers retain their identity; only `at_path` decides whether the
/// automatic modern-key admission is needed. No unresolved facet is filled here.
pub(crate) async fn prepare_at_path(ctx: &Ctx, conn: &mut Conn) -> ApiResult<Option<Prepared>> {
    let operation = operations::for_route(&ctx.route).map_err(ApiError::internal)?;
    if operation.delegation != Delegation::Business {
        return Err(refused());
    }
    let [variant] = operation.variants.as_slice() else {
        return Ok(None);
    };
    if !variant.name.is_empty()
        || variant.facets.iter().any(|facet| {
            matches!(
                facet.target,
                TargetResolver::Path(_)
                    | TargetResolver::TypedPath { .. }
                    | TargetResolver::Resolved(_)
                    | TargetResolver::ResolvedCollection(_)
            )
        })
    {
        return Ok(None);
    }
    let mut prepared = Prepared::new(ctx.route, &variant.operation)?;
    let bindings = targets::TargetBindings::default();
    for (index, facet) in variant.facets.iter().enumerate() {
        let resolved = targets::resolve(ctx, conn, facet, &bindings).await?;
        let kind = match facet.resources.as_slice() {
            [kind] => *kind,
            kinds => {
                let first = resolved.first().ok_or_else(refused)?.object.kind;
                if !kinds.contains(&first)
                    || resolved.iter().any(|target| target.object.kind != first)
                {
                    return Err(refused());
                }
                first
            }
        };
        prepared.bind(index, kind, &resolved)?;
    }
    Ok(Some(prepared))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_access_contract::key_scope::{KeyIdentityKind, KeyObjectRef, KeyScope};
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::operations::{FacetSpec, OperationSpec};
    use orch_api_contract::routes::{Access, Endpoint};
    use orch_types::Id;
    use serde_json::json;

    fn caller(operation: &str, read: bool) -> Caller {
        let actions = if read {
            json!(["write", "read"])
        } else {
            json!(["write"])
        };
        let scope: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"topic", "id":Id::from_u128(1), "reach":"subtree"},
            "allow":[{"operation":operation,"actions":actions}]
        }]}))
        .unwrap();
        Caller::token(
            Id::from_u128(9),
            orch_authz::TokenGrant::from_stored_scope(&[], None, Some(scope), None).unwrap(),
        )
    }

    fn topic() -> KeyResolvedTarget {
        KeyResolvedTarget {
            object: KeyObjectRef {
                kind: KeyTargetKind::Object(ObjectKind::Topic),
                id: Some(Id::from_u128(1)),
            },
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        }
    }

    #[test]
    fn a_topic_prefix_never_satisfies_the_normal_complete_operation_gate() {
        let route = orch_api_contract::nodes::TopicCreate::ROUTE;
        let mut plan = Prepared::new(route, "TopicCreate.with_thread").unwrap();
        let parent = KeyResolvedTarget {
            object: KeyObjectRef {
                kind: KeyTargetKind::Object(ObjectKind::Project),
                id: Some(Id::from_u128(1)),
            },
            node_ancestors: Vec::new(),
            document_ancestors: Vec::new(),
            credential_account: None,
        };
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Topic), &[parent])
            .unwrap();
        let prefix = plan.topic_creation_prefix().unwrap();
        assert!(prefix.is_topic_creation_prefix());
        assert!(
            prefix.admit(&Caller::session(Id::from_u128(9))).is_err(),
            "unresolved Thread/environment/source effects remain required even for the marked prefix"
        );
        assert!(
            Prepared::new(
                orch_api_contract::nodes::ProjectCreate::ROUTE,
                "ProjectCreate"
            )
            .unwrap()
            .topic_creation_prefix()
            .is_err(),
            "the internal prefix marker is not a generic partial-admission path"
        );
    }

    #[test]
    fn a_thread_identity_prefix_cannot_skip_its_dependent_creation_effects() {
        let mut plan = Prepared::new(
            orch_api_contract::nodes::ThreadCreate::ROUTE,
            "ThreadCreate",
        )
        .unwrap();
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Thread), &[topic()])
            .unwrap();
        let prefix = plan.thread_creation_prefix().unwrap();
        assert!(prefix.is_topic_creation_prefix());
        assert!(prefix.admit(&Caller::session(Id::from_u128(9))).is_err());
        assert!(
            Prepared::new(
                orch_api_contract::nodes::TopicCreate::ROUTE,
                "TopicCreate.create"
            )
            .unwrap()
            .thread_creation_prefix()
            .is_err(),
            "only the canonical ThreadCreate selection has this private prefix"
        );
    }

    #[test]
    fn required_effects_admit_before_a_future_response_object_exists() {
        const SPEC: OperationSpec = OperationSpec::facets(&[
            FacetSpec::required(
                KeyAction::Write,
                &[KeyTargetKind::Object(ObjectKind::Document)],
                TargetResolver::Resolved("destination"),
            ),
            FacetSpec::response_content(
                &[KeyTargetKind::Object(ObjectKind::Document)],
                TargetResolver::Resolved("created_document"),
            ),
        ]);
        let mut route = orch_api_contract::documents::DocumentOpsCreate::ROUTE;
        route.name = "CreationControl";
        route.access = Access::User;
        route.operation = Some(OperationSpec::receipt(&SPEC));
        let mut pending = Prepared::new(route, route.name).unwrap();
        pending
            .bind(0, KeyTargetKind::Object(ObjectKind::Document), &[topic()])
            .unwrap();
        let writer = caller(route.name, false);
        assert_eq!(
            pending.clone().admit(&writer).unwrap().disclosure,
            Disclosure::Pending
        );
        let created = KeyResolvedTarget {
            object: KeyObjectRef {
                kind: KeyTargetKind::Object(ObjectKind::Document),
                id: Some(Id::from_u128(2)),
            },
            node_ancestors: vec![topic().object],
            document_ancestors: Vec::new(),
            credential_account: None,
        };
        pending
            .bind(1, KeyTargetKind::Object(ObjectKind::Document), &[created])
            .unwrap();
        assert_eq!(
            pending.clone().admit(&writer).unwrap().disclosure,
            Disclosure::Denied
        );
        assert_eq!(
            pending.admit(&caller(route.name, true)).unwrap().disclosure,
            Disclosure::Full
        );
    }

    #[test]
    fn only_an_explicit_collection_can_be_checked_empty_and_missing_still_refuses() {
        const SPEC: OperationSpec = OperationSpec::facets(&[
            FacetSpec::required(
                KeyAction::Write,
                &[KeyTargetKind::Object(ObjectKind::Topic)],
                TargetResolver::Resolved("destination"),
            ),
            FacetSpec::required(
                KeyAction::Read,
                &[KeyTargetKind::Identity(KeyIdentityKind::Chat)],
                TargetResolver::ResolvedCollection("sources"),
            ),
        ]);
        let mut route = orch_api_contract::documents::DocumentOpsCreate::ROUTE;
        route.name = "CollectionControl";
        route.access = Access::User;
        route.operation = Some(SPEC);
        let mut plan = Prepared::new(route, route.name).unwrap();
        assert!(
            plan.bind_empty(0).is_err(),
            "a destination is not an empty collection"
        );
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Topic), &[topic()])
            .unwrap();
        assert!(plan.clone().admit(&caller(route.name, false)).is_err());
        plan.bind_empty(1).unwrap();
        assert!(
            plan.bind_empty(1).is_err(),
            "checked slots cannot be overwritten"
        );
        assert!(
            plan.admit(&caller(route.name, false)).is_ok(),
            "zero source reads need no invented Read grant"
        );
    }

    #[test]
    fn a_fully_empty_footprint_retains_exact_required_selection_evidence() {
        const SPEC: OperationSpec = OperationSpec::facets(&[FacetSpec::required(
            KeyAction::Manage,
            &[KeyTargetKind::Identity(KeyIdentityKind::Session)],
            TargetResolver::ResolvedCollection("selected_sessions"),
        )]);
        let mut route = orch_api_contract::auth::MeSessionOthersDelete::ROUTE;
        route.name = "EmptySessionsControl";
        route.access = Access::User;
        route.operation = Some(SPEC);
        let policy: KeyScope = serde_json::from_value(json!({"rules":[{
            "target":{"kind":"account","id":Id::from_u128(9),"reach":"subtree"},
            "allow":[{"operation":route.name,"actions":["manage"]}]
        }]}))
        .unwrap();
        let caller = Caller::token(
            Id::from_u128(9),
            orch_authz::TokenGrant::from_stored_scope(&[], None, Some(policy), None).unwrap(),
        );
        let mut plan = Prepared::new(route, route.name).unwrap();
        assert!(
            plan.clone().admit(&caller).is_err(),
            "missing is not checked empty"
        );
        plan.bind_empty(0).unwrap();
        let admitted = plan.admit(&caller).unwrap().caller;
        assert_eq!(admitted.admitted_operation(), Some(route.name));
        assert!(admitted.admitted_facets().unwrap().is_empty());
        assert!(checked_empty_for(&route, &admitted).unwrap());
        route.name = "AnotherOperation";
        assert!(!checked_empty_for(&route, &admitted).unwrap());
    }
}
