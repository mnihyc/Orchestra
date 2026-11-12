//! The same native original-operation frame for API-owned workers, without a request context.

use orch_engine::operation_scope::continuing as shared;

pub(crate) use shared::Error;

pub(crate) use shared::Source;

use crate::key_operations::admission;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::CurrentHttpIdentity;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::registry::Ctx;
/// Domain workers supply actual immutable native association and its existing source factory.
/// Known invalid original credentials remain errors and never fall back to ordinary work.
pub async fn prepare<F: Source>(
    conn: &mut orch_db::Conn,
    clock: &dyn orch_types::Clock,
    origin: &orch_authz::CapturedOperation,
    input: F::Input,
) -> Result<shared::Frame<F>, Error> {
    shared::prepare::<F>(conn, clock, origin, input).await
}

/// The actual raw HTTP and complete native caller minimum on the owner's still-open
/// transaction. This cut grants no permission and never commits, retries or opens source IO.
pub struct Deadline {
    app: crate::state::App,
    identity: CurrentHttpIdentity,
}

impl Deadline {
    pub fn check(&self) -> ApiResult<()> {
        self.identity.check_deadline(&self.app)
    }
}

fn source_error(error: Error) -> ApiError {
    match error {
        Error::Refused => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the original native source is no longer current",
        ),
        Error::Binding(error) => admission::binding_error(error),
        Error::Native(error) => crate::key_operations::snapshot_read::engine_error(error),
    }
}

async fn http_identity<F: Source>(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    origin: &orch_authz::CapturedOperation,
) -> ApiResult<CurrentHttpIdentity> {
    if !orch_db::queries::transaction_guard::in_transaction(conn)
        || ctx.route.name != F::ROUTE.name
        || ctx.route.method != F::ROUTE.method
        || ctx.route.path != F::ROUTE.path
    {
        return Err(ApiError::internal(
            "the native HTTP frame requires its exact operation and transaction",
        ));
    }
    orch_engine::operation_scope::Prepared::new(ctx.route, origin.operation())
        .map_err(admission::binding_error)?;
    let mut identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    transaction_guard::guard_user(&identity.user)?;
    let original = origin
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthInvalidToken,
                "the original native credential has ended",
            )
        })?;
    if !admission::same_response_origin(ctx, &original)? {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the native source changed its HTTP credential origin",
        ));
    }
    identity.caller = identity
        .caller
        .intersect_restrictions(&original)
        .map_err(ApiError::internal)?;
    identity.user.as_admin = identity.caller.as_admin();
    identity.check_deadline(&ctx.app)?;
    Ok(identity)
}

/// Compose the real request credential with the existing native captured Source on the
/// supplied owned transaction, after native/store waits and before its incidental first write.
/// The owner consumes both native first_effect and this raw deadline at its actual writer,
/// and checks the deadline again after later audit/commit preparation waits.
pub async fn prepare_on<F: Source>(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    origin: &orch_authz::CapturedOperation,
    input: F::Input,
) -> ApiResult<(shared::Frame<F>, Deadline)> {
    let mut identity = http_identity::<F>(ctx, conn, origin).await?;
    let frame = shared::prepare_with_caller::<F>(
        conn,
        ctx.app.clock.as_ref(),
        origin,
        &identity.caller,
        input,
    )
    .await
    .map_err(source_error)?;
    identity.caller = frame.principal().caller().clone();
    let deadline = Deadline {
        app: ctx.app.clone(),
        identity,
    };
    deadline.check()?;
    Ok((frame, deadline))
}

/// Refresh that same original immutable native input/value on an owned HTTP connection.
/// No registered handler/opener, selector window, source IO or renderer is replayed.
pub async fn refresh_on<F: Source>(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    opened: &shared::Opened<F>,
) -> ApiResult<(shared::Frame<F>, Deadline)> {
    let mut identity = http_identity::<F>(ctx, conn, opened.origin()).await?;
    let frame = opened
        .refresh_with_caller(conn, ctx.app.clock.as_ref(), &identity.caller)
        .await
        .map_err(source_error)?;
    identity.caller = frame.principal().caller().clone();
    let deadline = Deadline {
        app: ctx.app.clone(),
        identity,
    };
    deadline.check()?;
    Ok((frame, deadline))
}

/// Exact HTTP/original identity on the typed native known-source preparation phase.
/// The native owner still supplies its complete phase footprint and actual setup seams.
pub async fn prepare_initial_on<F: Source>(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    origin: &orch_authz::CapturedOperation,
    input: F::Input,
) -> ApiResult<(shared::preparation::Frame<F>, Deadline)> {
    let mut identity = http_identity::<F>(ctx, conn, origin).await?;
    let frame = shared::preparation::prepare_with_caller::<F>(
        conn,
        ctx.app.clock.as_ref(),
        origin,
        &identity.caller,
        input,
    )
    .await
    .map_err(source_error)?;
    identity.caller = frame.principal().caller().clone();
    let deadline = Deadline {
        app: ctx.app.clone(),
        identity,
    };
    deadline.check()?;
    Ok((frame, deadline))
}

#[path = "continuing/common.rs"]
pub mod common;
