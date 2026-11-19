//! Final transport checks for an already produced response, without replaying its operation.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, HttpIdentityMode};
use crate::registry::Ctx;
use axum::response::{IntoResponse, Response};
use futures::future::BoxFuture;
use orch_api_contract::common::ErrorCode;
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_types::Time;
use serde::Serialize;
use std::sync::{Arc, Mutex};

type Check = dyn for<'a> Fn(&'a Ctx, &'a mut Conn, &'a Principal) -> BoxFuture<'a, ApiResult<()>>
    + Send
    + Sync;

struct Source {
    accepted: Caller,
    check: Arc<Check>,
}

/// Request-local transport state, separate from replaceable operation preparation.
/// A deadline supplies no target/permission proof; a source check keeps its native association.
#[derive(Default)]
pub(crate) struct State {
    deadline: Mutex<Option<Time>>,
    sources: Mutex<Vec<Arc<Source>>>,
    receipt: Mutex<Option<Vec<u8>>>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Publication([opaque])")
    }
}

impl State {
    /// Only narrow a lifetime observed by an actual identity or accepted native boundary.
    pub(crate) fn retain_deadline(&self, end: Option<Time>) -> ApiResult<()> {
        if let Some(end) = end {
            let mut held = self
                .deadline
                .lock()
                .map_err(|_| ApiError::internal("response lifetime state is poisoned"))?;
            *held = Some(held.map_or(end, |old| old.min(end)));
        }
        Ok(())
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
pub(crate) fn receipt<T: Serialize>(ctx: &Ctx, receipt: &T) -> ApiResult<()> {
    if !orch_api_contract::operations::for_route(&ctx.route)
        .map_err(ApiError::internal)?
        .receipt
    {
        return Err(ApiError::internal("this operation has no typed receipt"));
    }
    *ctx.publication
        .receipt
        .lock()
        .map_err(|_| ApiError::internal("response receipt state is poisoned"))? =
        Some(serde_json::to_vec(receipt).map_err(ApiError::internal)?);
    Ok(())
}

/// Retain only an already selected native source and its effect-free current gate.
/// The callback must recheck that source's association and ordinary authority. It must
/// not select a replacement window/source, consume IO, dispatch work or repeat a writer.
pub(crate) fn retain<T, F>(ctx: &Ctx, accepted: &Caller, source: T, check: F) -> ApiResult<()>
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
    ctx.publication
        .sources
        .lock()
        .map_err(|_| ApiError::internal("response source state is poisoned"))?
        .push(Arc::new(Source {
            accepted: accepted.clone(),
            check: retained,
        }));
    Ok(())
}

/// Current original HTTP identity and each frozen native source, followed by a synchronous cut.
pub(crate) async fn check(ctx: &Ctx) -> ApiResult<()> {
    let sources = ctx
        .publication
        .sources
        .lock()
        .map_err(|_| ApiError::internal("response source state is poisoned"))?
        .clone();
    let key = ctx.user().is_ok_and(|user| !user.is_session());
    if !key && sources.is_empty() {
        return Ok(());
    }
    if ctx.surface.is_some() && (key || !sources.is_empty()) {
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
            let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
            (source.check)(&current, &mut tx, &principal).await?;
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
    match crate::response_body::checked(response, move || {
        let ctx = checking.clone();
        async move { check(&ctx).await }
    })
    .await
    {
        Ok(response) => Ok(response),
        Err(error) => {
            let receipt = ctx
                .publication
                .receipt
                .lock()
                .map_err(|_| ApiError::internal("response receipt state is poisoned"))?
                .clone();
            if status.is_success()
                && (matches!(error.problem.status, 401 | 403 | 404 | 410)
                    || error.is_database_retry())
                && let Some(receipt) = receipt
            {
                return Ok((
                    status,
                    [(http::header::CONTENT_TYPE, "application/json")],
                    receipt,
                )
                    .into_response());
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
            check(&ctx).await?;
            Err(error)
        }
        (None, result) => result,
    }
}
