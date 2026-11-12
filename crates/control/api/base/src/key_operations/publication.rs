//! Final transport checks for an already produced response, without replaying its operation.

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::registry::Ctx;

use axum::response::IntoResponse;

use axum::response::Response;

use bytes::Bytes;

use futures::future::BoxFuture;

use futures::Stream;

use futures::StreamExt;

use orch_api_contract::common::ErrorCode;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Time;

use serde::Serialize;

use std::sync::Arc;

use std::sync::Mutex;
type Check = dyn for<'a> Fn(&'a Ctx, &'a mut Conn, &'a Principal) -> BoxFuture<'a, ApiResult<()>>
    + Send
    + Sync;

struct Source {
    accepted: Caller,
    check: Arc<Check>,
}

#[derive(Clone)]
enum Fallback {
    Receipt(Vec<u8>),
    Refusal(ApiError),
}

/// Request-local transport state, separate from replaceable operation preparation.
/// A deadline supplies no target/permission proof; a source check keeps its native association.
#[derive(Default)]
pub struct State {
    deadline: Mutex<Option<Time>>,
    ceiling: Mutex<Option<orch_authz::CapturedCaller>>,
    sources: Mutex<Vec<Arc<Source>>>,
    fallback: Mutex<Option<Fallback>>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Publication([opaque])")
    }
}

impl State {
    fn retain_fallback(&self, fallback: Fallback) -> ApiResult<()> {
        let mut held = self
            .fallback
            .lock()
            .map_err(|_| ApiError::internal("response alternative state is poisoned"))?;
        match (&*held, &fallback) {
            (Some(Fallback::Receipt(_)), Fallback::Refusal(_))
            | (Some(Fallback::Refusal(_)), Fallback::Receipt(_)) => {
                return Err(ApiError::internal(
                    "a native refusal must be separate from accepted effects",
                ));
            }
            (Some(Fallback::Refusal(old)), Fallback::Refusal(new)) if !same_refusal(old, new) => {
                return Err(ApiError::internal(
                    "a native refusal cannot replace another failure",
                ));
            }
            _ => {}
        }
        *held = Some(fallback);
        Ok(())
    }

    fn fallback(&self) -> ApiResult<Option<Fallback>> {
        self.fallback
            .lock()
            .map(|held| held.clone())
            .map_err(|_| ApiError::internal("response alternative state is poisoned"))
    }

    /// Only narrow a lifetime observed by an actual identity or accepted native boundary.
    pub fn retain_deadline(&self, end: Option<Time>) -> ApiResult<()> {
        if let Some(end) = end {
            let mut held = self
                .deadline
                .lock()
                .map_err(|_| ApiError::internal("response lifetime state is poisoned"))?;
            *held = Some(held.map_or(end, |old| old.min(end)));
        }
        Ok(())
    }

    /// Accepted restrictions are shared across this opening's attempts, never target admission.
    pub fn retain_ceiling(&self, caller: &Caller) -> ApiResult<()> {
        let next = orch_authz::CapturedCaller::capture(caller);
        let mut held = self
            .ceiling
            .lock()
            .map_err(|_| ApiError::internal("response ceiling state is poisoned"))?;
        *held = Some(match held.as_ref() {
            Some(old) => old.intersect(&next).map_err(ApiError::internal)?,
            None => next,
        });
        Ok(())
    }
    pub fn retained_ceiling(&self) -> ApiResult<Option<orch_authz::CapturedCaller>> {
        self.ceiling
            .lock()
            .map(|held| held.clone())
            .map_err(|_| ApiError::internal("response ceiling state is poisoned"))
    }

    /// Earliest lifetime already observed by this actual HTTP operation.
    pub fn retained_deadline(&self) -> ApiResult<Option<Time>> {
        self.deadline
            .lock()
            .map(|held| *held)
            .map_err(|_| ApiError::internal("response lifetime state is poisoned"))
    }

    fn check_deadline(&self, ctx: &Ctx) -> ApiResult<()> {
        let end = *self
            .deadline
            .lock()
            .map_err(|_| ApiError::internal("response lifetime state is poisoned"))?;
        if end.is_some_and(|end| end <= ctx.app.clock.time()) {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the response credential is no longer valid",
            ));
        }
        Ok(())
    }
}

/// A native writer supplies its actual typed safe alternative after completion.
/// This never derives a receipt by removing fields from a full response.
pub fn receipt<T: Serialize>(ctx: &Ctx, receipt: &T) -> ApiResult<()> {
    if !orch_api_contract::operations::for_route(&ctx.route)
        .map_err(ApiError::internal)?
        .receipt
    {
        return Err(ApiError::internal("this operation has no typed receipt"));
    }
    ctx.publication.retain_fallback(Fallback::Receipt(
        serde_json::to_vec(receipt).map_err(ApiError::internal)?,
    ))
}

pub(crate) fn same_refusal(left: &ApiError, right: &ApiError) -> bool {
    left.problem.status == right.problem.status
        && left.problem.code == right.problem.code
        && left.retry_after == right.retry_after
        && left.is_database_retry() == right.is_database_retry()
}

/// Native failed work supplies its actual sparse same-error form before optional rich Read.
/// It is not a completed effect/receipt and never changes the error's status/code/retry hint.
pub fn refusal(ctx: &Ctx, sparse: &ApiError) -> ApiResult<()> {
    if sparse.problem.status < 400 {
        return Err(ApiError::internal(
            "a native refusal must retain its actual failure status",
        ));
    }
    ctx.publication
        .retain_fallback(Fallback::Refusal(sparse.clone()))
}

/// Retain only an already selected native source and its effect-free current gate.
/// The callback must recheck that source's association and ordinary authority. It must
/// not select a replacement window/source, consume IO, dispatch work or repeat a writer.
pub fn retain<T, F>(ctx: &Ctx, accepted: &Caller, source: T, check: F) -> ApiResult<()>
where
    T: Send + Sync + 'static,
    F: for<'a> Fn(&'a Ctx, &'a mut Conn, &'a Principal, &'a T) -> BoxFuture<'a, ApiResult<()>>
        + Send
        + Sync
        + 'static,
{
    let source = source_check(ctx, accepted, source, check)?;
    ctx.publication
        .sources
        .lock()
        .map_err(|_| ApiError::internal("response source state is poisoned"))?
        .push(source);
    Ok(())
}

fn source_check<T, F>(ctx: &Ctx, accepted: &Caller, source: T, check: F) -> ApiResult<Arc<Source>>
where
    T: Send + Sync + 'static,
    F: for<'a> Fn(&'a Ctx, &'a mut Conn, &'a Principal, &'a T) -> BoxFuture<'a, ApiResult<()>>
        + Send
        + Sync
        + 'static,
{
    if !super::admission::same_response_origin(ctx, accepted)? {
        return Err(ApiError::internal(
            "response source changed credential origin",
        ));
    }
    ctx.publication.retain_deadline(
        accepted
            .grant()
            .and_then(orch_authz::TokenGrant::expires_at),
    )?;
    let source = Arc::new(source);
    let check = Arc::new(check);
    let retained: Arc<Check> = Arc::new(move |ctx, conn, principal| {
        let source = Arc::clone(&source);
        let check = Arc::clone(&check);
        Box::pin(async move { check(ctx, conn, principal, &source).await })
    });
    Ok(Arc::new(Source {
        accepted: accepted.clone(),
        check: retained,
    }))
}

/// One immutable native header/page source. Clones retain the same original association;
/// no in-flight frame can replace it with a later page. Dropping it releases that source.
#[derive(Clone)]
pub struct Frame {
    publication: Arc<State>,
    route: orch_api_contract::routes::Route,
    surface: orch_api_contract::surface::HttpSurface,
    source: Arc<Source>,
}

/// Produced bytes and their actual already-opened native source travel together through
/// stream/backpressure waits. Neither this value nor Frame is reusable target admission.
pub struct FrameBytes {
    frame: Frame,
    bytes: Bytes,
}

impl Frame {
    pub fn data(&self, bytes: Bytes) -> FrameBytes {
        FrameBytes {
            frame: self.clone(),
            bytes,
        }
    }

    fn belongs_to(&self, ctx: &Ctx) -> bool {
        Arc::ptr_eq(&self.publication, &ctx.publication)
            && ctx.surface == Some(self.surface)
            && self.route.name == ctx.route.name
            && self.route.method == ctx.route.method
            && self.route.path == ctx.route.path
    }
}

/// Keep only this actual page/header and its effect-free current native gate, without
/// appending it to the request's permanent retained sources. The domain owns native
/// typed source binding/ordinary reading and original row/name association validation.
pub fn frame<T, F>(ctx: &Ctx, accepted: &Caller, source: T, check: F) -> ApiResult<Frame>
where
    T: Send + Sync + 'static,
    F: for<'a> Fn(&'a Ctx, &'a mut Conn, &'a Principal, &'a T) -> BoxFuture<'a, ApiResult<()>>
        + Send
        + Sync
        + 'static,
{
    let surface = ctx.surface.ok_or_else(|| {
        ApiError::internal("a streamed native source requires its actual HTTP origin")
    })?;
    if accepted
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
    {
        let selection = accepted.admitted_operation().ok_or_else(|| {
            ApiError::internal("a streamed native source needs its actual operation admission")
        })?;
        if !orch_api_contract::operations::for_route(&ctx.route)
            .map_err(ApiError::internal)?
            .variants
            .iter()
            .any(|variant| variant.operation == selection)
        {
            return Err(ApiError::internal(
                "a streamed source belongs to another operation",
            ));
        }
    }
    Ok(Frame {
        publication: Arc::clone(&ctx.publication),
        route: ctx.route,
        surface,
        source: source_check(ctx, accepted, source, check)?,
    })
}

#[derive(Clone)]
struct CheckedStream(Frame);

/// A native stream already owns its selected source. Its final header/page checks
/// replace re-running the raw operation's provisional resolver after opening IO.
pub(crate) fn has_stream_source(ctx: &Ctx, response: &Response) -> bool {
    response
        .extensions()
        .get::<CheckedStream>()
        .is_some_and(|CheckedStream(header)| header.belongs_to(ctx))
}

/// Preserve the native response parts and bounded stream. Each yielded chunk owns its
/// frozen page until the single outer current-identity/header/page check completes. No
/// global growing history, replacement page selection, rendering or source-IO retry occurs.
pub fn streaming_response<S>(
    ctx: &Ctx,
    response: Response,
    header: Frame,
    stream: S,
) -> ApiResult<Response>
where
    S: Stream<Item = ApiResult<FrameBytes>> + Send + 'static,
{
    if !header.belongs_to(ctx)
        || !matches!(
            ctx.route.reply,
            orch_api_contract::routes::Shape::Raw | orch_api_contract::routes::Shape::Bytes
        )
    {
        return Err(ApiError::internal(
            "a streamed header requires its original raw response declaration",
        ));
    }
    let (mut parts, _body) = response.into_parts();
    parts.extensions.insert(CheckedStream(header.clone()));
    let ctx = ctx.clone();
    let frames = futures::stream::try_unfold(
        (Box::pin(stream), ctx, header),
        |(mut stream, ctx, header)| async move {
            let Some(item) = stream.next().await else {
                return Ok(None);
            };
            let item = item?;
            if !item.frame.belongs_to(&ctx)
                || item.frame.source.accepted.admitted_operation()
                    != header.source.accepted.admitted_operation()
            {
                return Err(ApiError::internal(
                    "a streamed page changed its request origin",
                ));
            }
            let mut sources = retained_sources(&ctx)?;
            sources.push(Arc::clone(&header.source));
            if !Arc::ptr_eq(&item.frame.source, &header.source) {
                sources.push(Arc::clone(&item.frame.source));
            }
            check_sources(&ctx, &sources).await?;
            // No awaited bookkeeping follows this source check before the bytes leave.
            Ok(Some((item.bytes, (stream, ctx, header))))
        },
    );
    Ok(Response::from_parts(
        parts,
        axum::body::Body::from_stream(frames),
    ))
}

fn retained_sources(ctx: &Ctx) -> ApiResult<Vec<Arc<Source>>> {
    ctx.publication
        .sources
        .lock()
        .map(|sources| sources.clone())
        .map_err(|_| ApiError::internal("response source state is poisoned"))
}

/// Current original HTTP identity and each frozen native source, followed by a synchronous cut.
pub async fn check(ctx: &Ctx) -> ApiResult<()> {
    check_sources(ctx, &retained_sources(ctx)?).await
}

async fn check_sources(ctx: &Ctx, sources: &[Arc<Source>]) -> ApiResult<()> {
    super::snapshot_read::retry_conflict(|| check_sources_once(ctx, sources)).await
}

async fn check_sources_once(ctx: &Ctx, sources: &[Arc<Source>]) -> ApiResult<()> {
    let key = ctx.user().is_ok_and(|user| !user.is_session());
    if !key && sources.is_empty() {
        return Ok(());
    }
    let native_read = ctx
        .ask_read
        .as_ref()
        .is_some_and(|read| read.native_identity().is_some());
    if (ctx.surface.is_some() || native_read) && (key || !sources.is_empty()) {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let identity =
            transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read).await?;
        for source in sources {
            if source.accepted.as_admin() && !identity.caller.as_admin() {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the response's administrator context has ended",
                ));
            }
            let accepted = if source.accepted.grant().is_some() {
                orch_authz::CapturedCaller::capture(&source.accepted)
                    .restore_current(&mut tx, ctx.app.clock.as_ref())
                    .await
                    .map_err(authorization::restoration_error)?
                    .ok_or_else(|| {
                        ApiError::new(ErrorCode::AuthInvalidToken, "the response key has ended")
                    })?
            } else {
                source.accepted.clone()
            };
            let caller = identity
                .caller
                .intersect_restrictions(&accepted)
                .map_err(ApiError::internal)?;
            ctx.publication
                .retain_deadline(caller.grant().and_then(orch_authz::TokenGrant::expires_at))?;
            let principal = authorization::load_principal(&mut tx, caller, identity.now).await?;
            let principal =
                crate::sign_in::second_step::hold(&mut tx, &identity.user, principal).await?;
            let current = ctx
                .for_operation_checkpoint(identity.user.clone(), identity.now.get())
                .with_snapshot_inspection()?;
            let reads = current
                .inspection
                .as_ref()
                .map(|info| info.reads.clone())
                .unwrap_or_default();
            orch_db::queries::trace::read_scope(
                &reads,
                (source.check)(&current, &mut tx, &principal),
            )
            .await?;
        }
        tx.commit().await.map_err(DbError::from)?;
        identity.check_deadline(&ctx.app)?;
    }
    ctx.publication.check_deadline(ctx)
}

/// Called after Registry bookkeeping. Preserve completed effects and typed receipts;
/// a later frame wait repeats only the retained read gate, never operation preparation.
pub(crate) async fn response(ctx: Ctx, response: Response) -> ApiResult<Response> {
    if response.status().is_success()
        && matches!(ctx.route.reply, orch_api_contract::routes::Shape::None)
        && transaction_guard::changes(&ctx)
    {
        // The completed operation's native empty acknowledgement contains no existing content.
        return Ok(response);
    }
    let status = response.status();
    let checking = ctx.clone();
    let stream = response.extensions().get::<CheckedStream>().cloned();
    let checked = if let Some(CheckedStream(header)) = stream {
        if !header.belongs_to(&ctx) {
            return Err(ApiError::internal(
                "a checked stream changed its publication request",
            ));
        }
        let mut sources = retained_sources(&ctx)?;
        sources.push(Arc::clone(&header.source));
        // Its body performs the same complete check including the current immutable page.
        // Wrapping another awaited global-only check after it would stale that page cut.
        check_sources(&ctx, &sources).await.map(|()| response)
    } else {
        crate::response_body::checked(response, move || {
            let ctx = checking.clone();
            async move { check(&ctx).await }
        })
        .await
    };
    match checked {
        Ok(response) => Ok(response),
        Err(error) => {
            let fallback = ctx.publication.fallback()?;
            if status.is_success()
                && (matches!(error.problem.status, 401 | 403 | 404 | 410)
                    || error.is_database_retry())
                && let Some(Fallback::Receipt(receipt)) = &fallback
            {
                return Ok((
                    status,
                    [(http::header::CONTENT_TYPE, "application/json")],
                    receipt.clone(),
                )
                    .into_response());
            }
            if (matches!(error.problem.status, 401 | 403 | 404 | 410) || error.is_database_retry())
                && let Some(Fallback::Refusal(sparse)) = fallback
                && sparse.problem.status == status.as_u16()
            {
                return Ok(sparse.into_response_for(&ctx.request_id));
            }
            Err(error)
        }
    }
}

/// Error bodies share the final lifetime cut; successful bodies retain per-frame checks.
pub(crate) async fn result(ctx: Option<Ctx>, result: ApiResult<Response>) -> ApiResult<Response> {
    match (ctx, result) {
        (Some(ctx), Ok(response)) => self::response(ctx, response).await,
        (Some(ctx), Err(error)) => {
            if let Some(Fallback::Refusal(sparse)) = ctx.publication.fallback()?
                && same_refusal(&error, &sparse)
            {
                let response = error.into_response_for(&ctx.request_id);
                return self::response(ctx, response).await;
            }
            check(&ctx).await?;
            Err(error)
        }
        (None, result) => result,
    }
}

#[cfg(any(test, feature = "test-support"))]
#[allow(clippy::unwrap_used, reason = "a control fails by panicking")]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
    fn an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error() {
        let accepted = State::default();
        accepted
            .retain_fallback(Fallback::Receipt(b"actual receipt".to_vec()))
            .unwrap();
        let sparse = ApiError::new(ErrorCode::ConflictStaleRevision, "send a current revision");
        assert!(
            accepted
                .retain_fallback(Fallback::Refusal(sparse.clone()))
                .is_err()
        );
        assert!(
            matches!(accepted.fallback().unwrap(), Some(Fallback::Receipt(value)) if value.as_slice() == b"actual receipt")
        );

        let failed = State::default();
        failed
            .retain_fallback(Fallback::Refusal(sparse.clone()))
            .unwrap();
        assert!(
            failed
                .retain_fallback(Fallback::Receipt(b"not accepted".to_vec()))
                .is_err()
        );
        assert!(
            failed
                .retain_fallback(Fallback::Refusal(ApiError::not_found()))
                .is_err()
        );
        assert!(
            matches!(failed.fallback().unwrap(), Some(Fallback::Refusal(error)) if error == sparse)
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error()
    {
        self::an_unaccepted_refusal_never_replaces_an_accepted_receipt_or_a_different_error()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn sparse_failure_keeps_native_status_code_retry_and_trusted_cause() {
        let sparse = ApiError::new(ErrorCode::ConflictStaleRevision, "send a current revision")
            .retry_after(3);
        let rich = sparse
            .clone()
            .with("current", serde_json::json!({"revision":2}));
        assert!(same_refusal(&sparse, &rich));
        assert!(!same_refusal(&sparse, &rich.clone().retry_after(4)));
        let mut status = rich.clone();
        status.problem.status = 423;
        assert!(!same_refusal(&sparse, &status));
        assert!(!same_refusal(
            &sparse,
            &ApiError::not_found().retry_after(3)
        ));
        let retry: ApiError = DbError::Retry.into();
        let public_copy: ApiError = (*retry.problem).clone().into();
        assert_eq!(retry.problem, public_copy.problem);
        assert_eq!(retry.retry_after, public_copy.retry_after);
        assert!(!same_refusal(&retry, &public_copy));
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_sparse_failure_keeps_native_status_code_retry_and_trusted_cause() {
        self::sparse_failure_keeps_native_status_code_retry_and_trusted_cause()
    }
}
