//! The Snapshot producer's own current metadata checkpoints, shared by JSON, bytes and native pull.

use crate::middleware::{
    authorization::{self, refusals},
    requests::{ApiError, ApiResult},
};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use orch_access_contract::permissions::Permission;
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::Conn;
use orch_engine::archive::snapshot_authority::{Access, ReadWitness, Source};

pub(super) fn prepare<'a, Q>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let opened = ctx.optional_retained_source::<Opened>()?;
        let source = if let Some(opened) = &opened {
            if opened.origin.operation() != ctx.route.name || opened.row.id != ctx.id("id")? {
                return Err(ApiError::internal(
                    "opened Snapshot belongs to another operation",
                ));
            }
            Source::from_row(conn, opened.row.clone()).await?
        } else {
            Source::load(conn, ctx.id("id")?).await?
        }
        .ok_or_else(ApiError::not_found)?;
        let plan = source
            .prepare(conn, ctx.route.name)
            .await
            .map_err(engine_error)?
            .ok_or_else(ApiError::not_found)?;
        if let Some(opened) = &opened {
            opened_principal(ctx, conn, opened, &source, &plan, Some(caller)).await?;
        }
        Ok(OperationPlan::from_shared(plan))
    })
}

pub(super) fn engine_error(error: orch_engine::EngineError) -> ApiError {
    match error {
        orch_engine::EngineError::Db(error) => error.into(),
        orch_engine::EngineError::Authz(orch_authz::Error::Db(error)) => error.into(),
        other => ApiError::internal(other),
    }
}

/// Keep one unchanged ordinary decision implementation for existing reads and mutations.
pub(super) async fn require(
    conn: &mut Conn,
    principal: &Principal,
    source: &Source,
    permission: Permission,
) -> ApiResult<()> {
    match source.check(conn, principal, permission).await {
        Ok(Access::Allowed) => Ok(()),
        Ok(Access::MakerOnly) => Err(ApiError::not_found()),
        Ok(Access::Refused(refusal)) => Err(refusals::denied(conn, *refusal).await?),
        Err(error) => Err(authorization::engine_problem(conn, principal, error).await),
    }
}

/// All callers use an existing repeatable metadata snapshot. The Source loaded here is local;
/// concurrent archive/frame checks never borrow another checkpoint's shared operation-data slot.
pub(super) async fn in_snapshot(ctx: &Ctx, conn: &mut Conn) -> ApiResult<(Source, Principal)> {
    let principal = ctx.operation_principal(conn).await?;
    let source = Source::load(conn, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    require(conn, &principal, &source, Permission::FilesView).await?;
    Ok((source, principal))
}

/// Only effect-free metadata work can be retried. No native pull, recovery, store read or public
/// frame belongs in this scope; those await outside it and then re-enter a current checkpoint.
pub(super) async fn open(ctx: &Ctx, retain: bool) -> ApiResult<(Source, Read)> {
    crate::key_operations::snapshot_read::run(ctx.clone(), move |ctx| async move {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let (source, principal) = in_snapshot(&ctx, &mut tx).await?;
        let read = Read::new(&ctx, &source, &principal)?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if retain {
            read.publish()?;
        }
        Ok((source, read))
    })
    .await
}

#[derive(Clone, PartialEq)]
struct Opened {
    origin: CapturedOperation,
    row: orch_db::queries::storage::SnapshotRow,
}

/// Request-local continuing read. This retains only the real request context and immutable
/// Snapshot association; its current account/key/target admission is rebuilt for every use.
#[derive(Clone)]
pub(super) struct Read {
    ctx: Ctx,
    opened: Opened,
    witness: ReadWitness,
}

impl Read {
    pub(super) fn new(ctx: &Ctx, source: &Source, principal: &Principal) -> ApiResult<Self> {
        Ok(Self {
            ctx: ctx.clone(),
            opened: Opened {
                origin: CapturedOperation::capture(principal.caller())
                    .map_err(ApiError::internal)?,
                row: source.row.clone(),
            },
            witness: source.witness(),
        })
    }

    pub(super) async fn check(&self) -> ApiResult<()> {
        if *self.ctx.retained_source::<Opened>()? != self.opened {
            return Err(ApiError::internal("opened Snapshot source changed"));
        }
        let mut tx = self.ctx.app.db.begin_snapshot().await?;
        self.current_in(&mut tx).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(())
    }

    /// Registry return admission uses this authenticated association too. Fresh requests have
    /// no Opened data; replacement/deletion never retargets or revokes a response already opened.
    pub(super) fn publish(&self) -> ApiResult<()> {
        self.ctx.retain_source(self.opened.clone())
    }

    async fn admit(&self, conn: &mut Conn, source: Source) -> ApiResult<(Source, Principal)> {
        let plan = source
            .prepare(conn, self.opened.origin.operation())
            .await
            .map_err(engine_error)?
            .ok_or_else(ApiError::not_found)?;
        let principal =
            opened_principal(&self.ctx, conn, &self.opened, &source, &plan, None).await?;
        Ok((source, principal))
    }

    /// Current ordinary governor, derived from the original authenticated tuple. The manual
    /// branch still follows that original Thread's current Environment; no new manifest is read.
    pub(super) async fn current_in(&self, conn: &mut Conn) -> ApiResult<(Source, Principal)> {
        let source = Source::from_row(conn, self.opened.row.clone())
            .await?
            .ok_or_else(ApiError::not_found)?;
        self.admit(conn, source).await
    }

    /// A factual Get recovery may rebuild its own metadata result under the same original key
    /// ceiling. This never authorizes another native command or changes an opened byte stream.
    pub(super) async fn refreshed_in(&self, conn: &mut Conn) -> ApiResult<(Source, Principal)> {
        let source = Source::load(conn, self.opened.row.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        self.admit(conn, source).await
    }

    /// Before new native work, the exact current Snapshot must still be the recorded source.
    pub(super) async fn new_command_in(&self, conn: &mut Conn) -> ApiResult<(Source, Principal)> {
        let source = Source::load(conn, self.opened.row.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if !self.witness.matches(&source.row) {
            return Err(ApiError::not_found());
        }
        self.admit(conn, source).await
    }

    /// The shared transport-only adapter preserves the original response and each Frame.
    pub(super) async fn response(
        &self,
        response: axum::response::Response,
    ) -> ApiResult<axum::response::Response> {
        let read = std::sync::Arc::new(self.clone());
        crate::response_body::checked(response, move || {
            let read = std::sync::Arc::clone(&read);
            async move { read.check().await }
        })
        .await
    }
}

async fn opened_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    opened: &Opened,
    source: &Source,
    plan: &orch_engine::operation_scope::Prepared,
    current: Option<&Caller>,
) -> ApiResult<Principal> {
    // The captured operation is the immutable original ceiling, not a live
    // HTTP Session. Current request identity comes from the one shared boundary.
    let captured = opened
        .origin
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(|error| match error {
            orch_authz::Error::Db(error) => error.into(),
            other => ApiError::internal(other),
        })?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthInvalidToken,
                "the opened read credential is no longer valid",
            )
        })?;
    let identity = crate::middleware::transaction_guard::current_http_identity(
        ctx,
        conn,
        crate::middleware::transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    let mut caller = captured
        .intersect_restrictions(&identity.caller)
        .map_err(ApiError::internal)?;
    if let Some(current) = current {
        caller = caller
            .intersect_restrictions(current)
            .map_err(ApiError::internal)?;
    }
    let mut user = identity.user;
    // An opened ceiling may be stricter than the session's current requested
    // mode. Keep that narrowing without manufacturing a different Session.
    if caller.as_admin() && !user.as_admin {
        return Err(ApiError::internal(
            "opened Snapshot elevated its HTTP context",
        ));
    }
    user.as_admin = caller.as_admin();
    if !authorization::same_credential_origin(&user, &caller) {
        return Err(ApiError::internal(
            "opened Snapshot changed authenticated credential origin",
        ));
    }
    let caller = plan
        .admit(&caller, true)
        .map_err(crate::key_operations::admission::binding_error)?;
    if !caller.key_deadline_allows(ctx.app.clock.as_ref()) {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthInvalidToken,
            "the opened read credential is no longer valid",
        ));
    }
    let principal = authorization::load_principal(conn, caller, identity.now).await?;
    let principal = crate::sign_in::second_step::hold(conn, &user, principal).await?;
    let ordinary = require(conn, &principal, source, Permission::FilesView).await;
    // This is the last synchronous boundary after the ordinary reader's awaits.
    // The frozen source is not a credential lifetime and cannot waive either end.
    if identity
        .expires_at
        .is_some_and(|end| end <= ctx.app.clock.time())
        || !principal
            .caller()
            .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthInvalidToken,
            "the opened read credential is no longer valid",
        ));
    }
    ordinary?;
    Ok(principal)
}
