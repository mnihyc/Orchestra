//! Request-local typed operation preparation, reused at the actual transaction checkpoint.
//!
//! Only contract decoding lives here. Variant selection, source identity and ordinary
//! authorization remain in the endpoint's existing shared producers. Inputs and prepared
//! operands are never serialized, logged, or captured for deferred work.

use std::any::Any;
use std::fmt;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use orch_api_contract::common::{ErrorCode, MAX_JSON_BYTES};
use orch_api_contract::routes::Route;
use orch_authz::Caller;
use orch_db::Conn;
use serde::de::DeserializeOwned;

use super::admission::{Admission, Prepared};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{ConfiguredOperation, Ctx, OptionalOperationFuture, decode_body, read_query};

/// A disclosure still awaiting actual targets is not a proven refusal. A refused
/// Read can never be widened by a later checkpoint in the same request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Disclosure {
    Full = 0,
    Pending = 1,
    Denied = 2,
}

/// Shared by request/reply clones. Only a completed response checkpoint may make a
/// Pending response Full; Denied remains terminal.
#[derive(Debug, Default)]
pub(crate) struct DisclosureState(AtomicU8);

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
pub(crate) struct OperationData(Option<Arc<dyn Any + Send + Sync>>);

impl fmt::Debug for OperationData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperationData([redacted])")
    }
}

impl OperationData {
    pub(crate) fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Some(Arc::new(value)))
    }

    pub(crate) fn get<T: Any + Send + Sync>(&self) -> ApiResult<Arc<T>> {
        self.optional::<T>()?.ok_or_else(|| {
            ApiError::internal("the operation's current prepared operands are unavailable")
        })
    }

    pub(crate) fn optional<T: Any + Send + Sync>(&self) -> ApiResult<Option<Arc<T>>> {
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
pub(crate) struct DataSlot(Mutex<OperationData>);

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
pub(crate) struct RetainedSource(Mutex<OperationData>);

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

/// An erased typed resolver and its original bounded inputs, held only by this request.
#[derive(Clone)]
pub(crate) struct OperationRequest(Arc<dyn Resolve>);

impl fmt::Debug for OperationRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("OperationRequest([redacted])")
    }
}

impl OperationRequest {
    pub(crate) fn snapshot_read(&self) -> bool {
        self.0.snapshot_read()
    }

    pub(crate) fn new<
        Q: DeserializeOwned + Send + 'static,
        B: DeserializeOwned + Send + 'static,
    >(
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

    pub(crate) async fn prepare(
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
pub(crate) enum Resolution {
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
pub(crate) async fn resolve(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<Option<Resolution>> {
    ctx.clear_operation_data()?;
    match ctx.operation_request() {
        Some(request) => match request.prepare(ctx, conn, caller).await? {
            Some(prepared) if prepared.is_topic_creation_prefix() => {
                super::creation_stage::prepare_prefix(ctx, conn, caller, &prepared)
                    .await
                    .map(Resolution::TopicCreationPrefix)
                    .map(Some)
            }
            Some(prepared) => prepared.admit(caller).map(Resolution::Complete).map(Some),
            None => {
                ordinary_unclassified(caller)?;
                Ok(None)
            }
        },
        None => super::admission::at_path(ctx, conn, caller)
            .await
            .map(|admission| admission.map(Resolution::Complete)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
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
}
