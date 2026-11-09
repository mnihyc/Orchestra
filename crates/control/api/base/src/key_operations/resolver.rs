//! Request-local typed operation preparation, reused at the actual transaction checkpoint.
//!
//! Only contract decoding lives here. Variant selection, source identity and ordinary
//! authorization remain in the endpoint's existing shared producers. Inputs and prepared
//! operands are never serialized, logged, or captured for deferred work.

use std::any::Any;

use std::fmt;

use std::sync::atomic::AtomicU8;

use std::sync::atomic::Ordering;

use std::sync::Arc;

use std::sync::Mutex;

use bytes::Bytes;

use futures::future::BoxFuture;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::MAX_JSON_BYTES;

use orch_api_contract::routes::Method;

use orch_api_contract::routes::Route;

use orch_api_contract::routes::Shape;

use orch_authz::Caller;

use orch_db::Conn;

use serde::de::DeserializeOwned;

use super::admission::Admission;

use super::admission::Prepared;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::ConfiguredOperation;

use crate::registry::Ctx;

use crate::registry::OperationResolver;

use crate::registry::OptionalOperationFuture;

use crate::registry::OptionalOperationResolver;

use crate::registry::decode_body;

use crate::registry::read_query;
/// A disclosure still awaiting actual targets is not a proven refusal. A refused
/// Read can never be widened by a later checkpoint in the same request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Disclosure {
    Full = 0,
    Pending = 1,
    Denied = 2,
}

/// Shared by request/reply clones. Only a completed response checkpoint may make a
/// Pending response Full; Denied remains terminal.
#[derive(Debug, Default)]
pub struct DisclosureState(AtomicU8);

impl DisclosureState {
    pub(crate) fn current(&self) -> Disclosure {
        match self.0.load(Ordering::Acquire) {
            0 => Disclosure::Full,
            1 => Disclosure::Pending,
            _ => Disclosure::Denied,
        }
    }

    pub(crate) fn checkpoint(&self, next: Disclosure) {
        self.0.fetch_max(next as u8, Ordering::AcqRel);
    }

    pub(crate) fn complete(&self, next: Disclosure) {
        if next != Disclosure::Full {
            self.checkpoint(next);
            return;
        }
        let _ = self.0.compare_exchange(
            Disclosure::Pending as u8,
            Disclosure::Full as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Typed effect operands from the same preparation that established the footprint.
/// This deliberately has no Serialize implementation or value-bearing Debug output.
#[derive(Clone, Default)]
pub struct OperationData(Option<Arc<dyn Any + Send + Sync>>);

impl fmt::Debug for OperationData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperationData([redacted])")
    }
}

impl OperationData {
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Some(Arc::new(value)))
    }

    pub fn get<T: Any + Send + Sync>(&self) -> ApiResult<Arc<T>> {
        self.optional::<T>()?.ok_or_else(|| {
            ApiError::internal("the operation's current prepared operands are unavailable")
        })
    }

    pub fn optional<T: Any + Send + Sync>(&self) -> ApiResult<Option<Arc<T>>> {
        self.0
            .clone()
            .map(|value| {
                value.downcast::<T>().map_err(|_| {
                    ApiError::internal("the operation's current prepared operands are unavailable")
                })
            })
            .transpose()
    }
}

/// Shared read-preparation handoff. Mutations use the Guard's own data snapshot.
#[derive(Debug, Default)]
pub struct DataSlot(Mutex<OperationData>);

impl DataSlot {
    pub(crate) fn snapshot(&self) -> ApiResult<OperationData> {
        self.0
            .lock()
            .map(|data| data.clone())
            .map_err(|_| ApiError::internal("operation preparation state is poisoned"))
    }

    pub(crate) fn replace(&self, data: OperationData) -> ApiResult<()> {
        *self
            .0
            .lock()
            .map_err(|_| ApiError::internal("operation preparation state is poisoned"))? = data;
        Ok(())
    }

    pub(crate) fn get<T: Any + Send + Sync>(&self) -> ApiResult<Arc<T>> {
        self.0
            .lock()
            .map_err(|_| ApiError::internal("operation preparation state is poisoned"))?
            .get()
    }
}

/// One authenticated opened-source association for this request, independent of refreshed
/// prepared operands. It is never permission evidence and can only be retained unchanged.
#[derive(Default)]
pub struct RetainedSource(Mutex<OperationData>);

impl fmt::Debug for RetainedSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RetainedSource([opaque])")
    }
}

impl RetainedSource {
    pub(crate) fn retain<T: Any + Send + Sync + PartialEq>(&self, value: T) -> ApiResult<()> {
        let mut held = self
            .0
            .lock()
            .map_err(|_| ApiError::internal("opened source state is poisoned"))?;
        if let Some(current) = held.optional::<T>()? {
            if *current != value {
                return Err(ApiError::internal(
                    "an authenticated opened source cannot be replaced",
                ));
            }
        } else {
            *held = OperationData::new(value);
        }
        Ok(())
    }

    pub(crate) fn get<T: Any + Send + Sync>(&self) -> ApiResult<Arc<T>> {
        self.0
            .lock()
            .map_err(|_| ApiError::internal("opened source state is poisoned"))?
            .get()
    }

    pub(crate) fn optional<T: Any + Send + Sync>(&self) -> ApiResult<Option<Arc<T>>> {
        self.0
            .lock()
            .map_err(|_| ApiError::internal("opened source state is poisoned"))?
            .optional()
    }
}

trait Resolve: Send + Sync {
    fn snapshot_read(&self) -> bool;
    fn query_text(&self) -> Option<&str>;

    fn prepare<'a>(
        &'a self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
    ) -> OptionalOperationFuture<'a>;
}

struct TypedRequest<Q, B> {
    route: Route,
    query: Option<String>,
    body: Bytes,
    resolver: ConfiguredOperation<Q, B>,
}

impl<Q: DeserializeOwned + Send + 'static, B: DeserializeOwned + Send + 'static> Resolve
    for TypedRequest<Q, B>
{
    fn query_text(&self) -> Option<&str> {
        self.query.as_deref()
    }

    fn snapshot_read(&self) -> bool {
        self.resolver.snapshot_read()
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
    ) -> OptionalOperationFuture<'a> {
        Box::pin(async move {
            // The handler and every checkpoint use the same contract decoders and original
            // bytes: absent fields, explicit nulls and field-local query defaults agree.
            let query = read_query::<Q>(&self.route, self.query.as_deref())?;
            let body = decode_body::<B>(&self.route, &self.body)?;
            match self.resolver {
                ConfiguredOperation::Required(resolver)
                | ConfiguredOperation::SnapshotRequired(resolver) => {
                    resolver(ctx, conn, caller, query, body).await.map(Some)
                }
                ConfiguredOperation::Optional(resolver)
                | ConfiguredOperation::SnapshotOptional(resolver) => {
                    resolver(ctx, conn, caller, query, body).await
                }
            }
        })
    }
}

/// A changing raw operation whose footprint is wholly determined by its native subject/query.
/// The handler owns the stream once. Empty JSON bytes are never a substitute for that body.
struct RawQueryRequest<Q> {
    route: Route,
    query: Option<String>,
    resolver: OptionalOperationResolver<Q, ()>,
}

impl<Q: DeserializeOwned + Send + 'static> RawQueryRequest<Q> {
    fn query(&self) -> ApiResult<Q> {
        read_query(&self.route, self.query.as_deref())
    }
}

impl<Q: DeserializeOwned + Send + 'static> Resolve for RawQueryRequest<Q> {
    fn query_text(&self) -> Option<&str> {
        self.query.as_deref()
    }

    fn snapshot_read(&self) -> bool {
        false
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
    ) -> OptionalOperationFuture<'a> {
        Box::pin(async move { (self.resolver)(ctx, conn, caller, self.query()?, ()).await })
    }
}

/// The actual native bounded body result, immutable across every later guarded replan.
/// No Serialize/Deserialize/Clone implementation or body replay is required of the input.
struct RawOwnedRequest<Q, I> {
    route: Route,
    query: Option<String>,
    input: Arc<I>,
    accepted: orch_authz::CapturedCaller,
    known: orch_engine::operation_scope::Prepared,
    resolver: OperationResolver<Q, Arc<I>>,
}

impl<Q, I> Resolve for RawOwnedRequest<Q, I>
where
    Q: DeserializeOwned + Send + 'static,
    I: Send + Sync + 'static,
{
    fn query_text(&self) -> Option<&str> {
        self.query.as_deref()
    }

    fn snapshot_read(&self) -> bool {
        false
    }

    fn prepare<'a>(
        &'a self,
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
    ) -> OptionalOperationFuture<'a> {
        Box::pin(async move {
            let original = self
                .accepted
                .restore_current(conn, ctx.app.clock.as_ref())
                .await
                .map_err(crate::middleware::authorization::restoration_error)?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::AuthInvalidToken,
                        "the raw input credential has ended",
                    )
                })?;
            let caller = caller
                .intersect_restrictions(&original)
                .map_err(ApiError::internal)?;
            let query = read_query(&self.route, self.query.as_deref())?;
            let plan = (self.resolver)(ctx, conn, &caller, query, Arc::clone(&self.input)).await?;
            plan.shared()
                .required_facets()
                .map_err(super::admission::binding_error)?;
            if !plan.shared().covers_known_required_sources(&self.known) {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the raw input destination changed",
                ));
            }
            plan.within_caller(ctx, &caller).map(Some)
        })
    }
}

/// An erased typed resolver and its original bounded inputs, held only by this request.
#[derive(Clone)]
pub struct OperationRequest(Arc<dyn Resolve>);

impl fmt::Debug for OperationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperationRequest([redacted])")
    }
}

impl OperationRequest {
    /// One native decoder result and its original known destination/current caller ceiling.
    /// The prefix is factual input provenance; this request still requires full fresh admission.
    pub fn raw_owned<Q, I>(
        route: Route,
        resolver: OperationResolver<Q, Arc<I>>,
        query: Option<&str>,
        input: Arc<I>,
        known: orch_engine::operation_scope::Prepared,
        accepted: &Caller,
    ) -> ApiResult<Self>
    where
        Q: DeserializeOwned + Send + 'static,
        I: Send + Sync + 'static,
    {
        if route.method == Method::GET
            || !matches!(route.body, Shape::Bytes | Shape::Raw)
            || !known.matches_route(&route)
            || !known.covers_known_required_sources(&known)
        {
            return Err(ApiError::internal(
                "a raw body stage requires its actual known destination",
            ));
        }
        known
            .compatible(accepted)
            .map_err(super::admission::binding_error)?;
        if query.is_some_and(|query| query.len() > MAX_JSON_BYTES) {
            return Err(ApiError::new(
                ErrorCode::RequestTooLarge,
                "the operation input is too large",
            )
            .with("limit", MAX_JSON_BYTES));
        }
        Ok(Self(Arc::new(RawOwnedRequest {
            route,
            query: query.map(str::to_owned),
            input,
            accepted: orch_authz::CapturedCaller::capture(accepted),
            known,
            resolver,
        })))
    }

    /// Explicit query-only raw mutation input, not a typed JSON request with a fictional body.
    pub fn raw_query<Q: DeserializeOwned + Send + 'static>(
        route: Route,
        resolver: OptionalOperationResolver<Q, ()>,
        query: Option<&str>,
    ) -> ApiResult<Self> {
        if route.method == Method::GET || !matches!(route.body, Shape::Bytes | Shape::Raw) {
            return Err(ApiError::internal(
                "a raw changing query operation requires its actual raw body declaration",
            ));
        }
        if query.is_some_and(|query| query.len() > MAX_JSON_BYTES) {
            return Err(ApiError::new(
                ErrorCode::RequestTooLarge,
                "the operation input is too large",
            )
            .with("limit", MAX_JSON_BYTES));
        }
        Ok(Self(Arc::new(RawQueryRequest {
            route,
            query: query.map(str::to_owned),
            resolver,
        })))
    }

    pub fn snapshot_read(&self) -> bool {
        self.0.snapshot_read()
    }
    pub fn query_text(&self) -> Option<&str> {
        self.0.query_text()
    }

    pub fn new<Q: DeserializeOwned + Send + 'static, B: DeserializeOwned + Send + 'static>(
        route: Route,
        resolver: ConfiguredOperation<Q, B>,
        query: Option<&str>,
        body: Bytes,
    ) -> ApiResult<Self> {
        // Body already obeys MAX_JSON_BYTES. Reuse that existing structured-input budget
        // for the retained query instead of introducing another arbitrary retention limit.
        if query.is_some_and(|query| query.len() > MAX_JSON_BYTES) || body.len() > MAX_JSON_BYTES {
            return Err(ApiError::new(
                ErrorCode::RequestTooLarge,
                "the operation input is too large",
            )
            .with("limit", MAX_JSON_BYTES));
        }
        Ok(Self(Arc::new(TypedRequest {
            route,
            resolver,
            query: query.map(str::to_owned),
            body,
        })))
    }

    pub async fn prepare(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        caller: &Caller,
    ) -> ApiResult<Option<Prepared>> {
        let prepared = self.0.prepare(ctx, conn, caller).await?;
        if prepared
            .as_ref()
            .is_some_and(|prepared| !prepared.matches_route(&ctx.route))
        {
            return Err(ApiError::internal(
                "the operation resolver returned another endpoint's preparation",
            ));
        }
        Ok(prepared)
    }
}

/// Ordinary admission or the one explicitly staged compound creator. A prefix is not a Caller
/// admission and cannot be installed where an accepted operation is required.
pub enum Resolution {
    Complete(Admission),
    TopicCreationPrefix(super::creation_stage::TopicPrefix),
}

impl Resolution {
    pub(crate) fn complete(self) -> ApiResult<Admission> {
        match self {
            Self::Complete(admission) => Ok(admission),
            Self::TopicCreationPrefix(_) => Err(ApiError::internal(
                "compound topic creation needs its private transactional stage",
            )),
        }
    }
}

/// No declaration means no proof. Only the existing ordinary/legacy path may continue.
pub(crate) fn ordinary_unclassified(caller: &Caller) -> ApiResult<()> {
    if caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
        || caller.admitted_operation().is_some()
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the operation has no current declaration",
        ));
    }
    Ok(())
}

/// The registered typed producer wins; static metadata is only the existing fallback
/// for routes whose complete targets are known without request input.
pub(crate) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
) -> BoxFuture<'a, ApiResult<Option<Resolution>>> {
    Box::pin(async move {
        ctx.clear_operation_data()?;
        match ctx.operation_request() {
            Some(request) => match request.prepare(ctx, conn, caller).await? {
                Some(prepared) if prepared.is_topic_creation_prefix() => {
                    super::creation_stage::prepare_prefix(ctx, conn, caller, &prepared)
                        .await
                        .map(Resolution::TopicCreationPrefix)
                        .map(Some)
                }
                Some(prepared) => {
                    crate::ask::tools::check_plan(ctx, conn, &prepared).await?;
                    prepared.admit(caller).map(Resolution::Complete).map(Some)
                }
                None => {
                    if ctx.ask_read.is_some() {
                        return Err(ApiError::new(
                            ErrorCode::AuthzDenied,
                            "this read lacks a complete current source binding",
                        ));
                    }
                    ordinary_unclassified(caller)?;
                    Ok(None)
                }
            },
            None => super::admission::at_path(ctx, conn, caller)
                .await
                .map(|admission| admission.map(Resolution::Complete)),
        }
    })
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    fn raw_query_source<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _caller: &'a Caller,
        _query: orch_api_contract::threads::UploadSessionPutQuery,
        (): (),
    ) -> OptionalOperationFuture<'a> {
        Box::pin(async { Ok(None) })
    }

    #[cfg(any(test, feature = "test-support"))]
    fn raw_query_keeps_the_real_byte_body_and_native_offset_decoder() {
        use orch_api_contract::routes::Endpoint;
        use orch_api_contract::threads::{UploadSessionPut, UploadSessionPutQuery};
        let route = UploadSessionPut::ROUTE;
        assert!(matches!(route.body, Shape::Bytes));
        assert!(decode_body::<()>(&route, &[]).is_err());
        for (text, offset) in [("offset=0", 0), ("offset=17", 17), ("offset=-1", -1)] {
            let request = RawQueryRequest::<UploadSessionPutQuery> {
                route,
                query: Some(text.to_owned()),
                resolver: raw_query_source,
            };
            assert_eq!(request.query().unwrap().offset, offset);
            assert!(!request.snapshot_read());
            let erased = OperationRequest::raw_query(route, raw_query_source, Some(text)).unwrap();
            assert!(!erased.snapshot_read());
            assert!(!format!("{erased:?}").contains(text));
        }
        for query in [None, Some("offset=not-a-number")] {
            let request = RawQueryRequest::<UploadSessionPutQuery> {
                route,
                query: query.map(str::to_owned),
                resolver: raw_query_source,
            };
            assert!(request.query().is_err());
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_raw_query_keeps_the_real_byte_body_and_native_offset_decoder() {
        self::raw_query_keeps_the_real_byte_body_and_native_offset_decoder()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query() {
        use orch_api_contract::routes::Endpoint;
        let route = orch_api_contract::threads::UploadSessionPut::ROUTE;
        let mut fictional = route;
        fictional.body = Shape::None;
        assert!(
            OperationRequest::raw_query(fictional, raw_query_source, Some("offset=0")).is_err()
        );
        fictional = route;
        fictional.method = Method::GET;
        assert!(
            OperationRequest::raw_query(fictional, raw_query_source, Some("offset=0")).is_err()
        );
        let oversized = "x".repeat(MAX_JSON_BYTES + 1);
        let error =
            OperationRequest::raw_query(route, raw_query_source, Some(&oversized)).unwrap_err();
        assert_eq!(error.code(), ErrorCode::RequestTooLarge);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query() {
        self::raw_query_rejects_an_invented_body_read_mode_or_unbounded_retained_query()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn retained_opened_source_survives_fresh_preparation_and_never_retargets() {
        let prepared = DataSlot::default();
        let retained = RetainedSource::default();
        assert!(retained.get::<String>().is_err());
        assert!(retained.optional::<String>().unwrap().is_none());
        retained
            .retain("authenticated private association".to_owned())
            .unwrap();
        prepared
            .replace(OperationData::new("old target facts".to_owned()))
            .unwrap();
        prepared.replace(OperationData::default()).unwrap();
        assert!(prepared.get::<String>().is_err());
        retained
            .retain("authenticated private association".to_owned())
            .unwrap();
        assert!(
            retained
                .retain("replacement association".to_owned())
                .is_err()
        );
        assert!(retained.retain(17_u64).is_err());
        assert!(retained.get::<u64>().is_err());
        assert_eq!(
            retained.get::<String>().unwrap().as_str(),
            "authenticated private association"
        );
        assert!(!format!("{retained:?}").contains("private"));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_retained_opened_source_survives_fresh_preparation_and_never_retargets() {
        self::retained_opened_source_survives_fresh_preparation_and_never_retargets()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn poisoned_opened_source_cannot_downgrade_to_unopened() {
        let retained = RetainedSource::default();
        let failed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = retained.0.lock().unwrap();
            panic!("fixture deliberately poisons its source slot");
        }));
        assert!(failed.is_err());
        assert!(retained.get::<String>().is_err());
        assert!(retained.optional::<String>().is_err());
        assert!(retained.retain("new association".to_owned()).is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_poisoned_opened_source_cannot_downgrade_to_unopened() {
        self::poisoned_opened_source_cannot_downgrade_to_unopened()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof() {
        let user = orch_types::new_id();
        let ordinary = Caller::session(user);
        assert!(ordinary_unclassified(&ordinary).is_ok());
        assert!(ordinary.admitted_facets().is_none());
        let key = Caller::token(
            user,
            orch_authz::TokenGrant::from_stored_scope(
                &[],
                None,
                Some(orch_access_contract::key_scope::KeyScope::default()),
                None,
            )
            .unwrap(),
        );
        assert!(ordinary_unclassified(&key).is_err());
        assert!(key.admitted_facets().is_none());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof()
     {
        self::unclassified_ordinary_path_never_admits_a_modern_key_or_retains_operation_proof()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn pending_completion_is_explicit_and_denied_never_widens() {
        let state = DisclosureState::default();
        assert_eq!(state.current(), Disclosure::Full);
        state.checkpoint(Disclosure::Pending);
        state.checkpoint(Disclosure::Full);
        assert_eq!(
            state.current(),
            Disclosure::Pending,
            "a later effect checkpoint is not Read completion"
        );
        state.complete(Disclosure::Full);
        assert_eq!(state.current(), Disclosure::Full);
        state.checkpoint(Disclosure::Denied);
        state.complete(Disclosure::Full);
        state.checkpoint(Disclosure::Pending);
        assert_eq!(state.current(), Disclosure::Denied);
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_pending_completion_is_explicit_and_denied_never_widens() {
        self::pending_completion_is_explicit_and_denied_never_widens()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement() {
        let before = OperationData::new("private first operand".to_owned());
        let slot = DataSlot::default();
        slot.replace(before.clone()).unwrap();
        slot.replace(OperationData::new("private second operand".to_owned()))
            .unwrap();
        assert_eq!(
            before.get::<String>().unwrap().as_str(),
            "private first operand"
        );
        assert_eq!(
            slot.get::<String>().unwrap().as_str(),
            "private second operand"
        );
        assert!(!format!("{slot:?}").contains("private"));
        slot.replace(OperationData::default()).unwrap();
        assert!(slot.get::<String>().is_err());
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement()
     {
        self::prepared_operands_are_redacted_and_guard_snapshots_do_not_follow_later_replacement()
    }
}
