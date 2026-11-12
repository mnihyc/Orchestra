//! Capture the authenticator's real raw identity for native sources and stored commands.

use crate::middleware::authentication;

use crate::middleware::authentication::Via;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::middleware::transaction_guard::HttpIdentityMode;

use crate::registry::Ctx;

use orch_authz::CapturedOperation;

use orch_db::Conn;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::http_origin::Current;

use orch_engine::operation_scope::http_origin::HttpOrigin;

use orch_engine::operation_scope::request_origin::Credential;
fn mapped(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Native(error) => ApiError::engine(error),
        _ => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "The original HTTP identity no longer permits this native boundary.",
        ),
    }
}

fn credential(ctx: &Ctx) -> ApiResult<Credential> {
    let surface = ctx.http_surface()?;
    authentication::on_surface(surface, &ctx.headers)?;
    let original = ctx.authenticated_user()?;
    let hash = authentication::bearer(&ctx.headers)
        .map(authentication::token_hash)
        .ok_or_else(|| {
            ApiError::new(
                orch_api_contract::common::ErrorCode::AuthInvalidToken,
                "The original HTTP bearer is missing.",
            )
        })?;
    Ok(match original.via {
        Via::OwnerRead { .. } => {
            return Err(ApiError::internal(
                "a native owner read has no raw HTTP preparation credential",
            ));
        }
        Via::Session { session_id } => Credential::Session {
            id: session_id,
            hash,
        },
        Via::Token { token_id, .. } => Credential::Token { id: token_id, hash },
    })
}

/// Capture the actual raw preparation identity, with no complete operation assertion.
/// The native supplier still binds and checks its canonical known sources on every use.
pub(crate) async fn capture_preparation(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &orch_authz::Caller,
) -> ApiResult<orch_authz::http_origin::HttpPreparation> {
    let original = ctx.authenticated_user()?;
    if caller.user() != original.id() {
        return Err(ApiError::internal(
            "The native preparation changed its HTTP actor.",
        ));
    }
    let identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    transaction_guard::guard_user(&identity.user)?;
    let caller = caller
        .intersect_restrictions(&identity.caller)
        .map_err(ApiError::internal)?;
    let until = ctx
        .publication
        .retained_deadline()?
        .into_iter()
        .chain(identity.expires_at)
        .min();
    let source = orch_authz::http_origin::HttpPreparation::capture(
        ctx.route.name,
        &caller,
        credential(ctx)?,
        original.as_admin,
        original.user.is_system_admin,
        until,
    )
    .map_err(preparation_error)?;
    current_preparation(ctx, conn, &source).await?;
    Ok(source)
}

fn preparation_error(error: orch_authz::http_origin::Error) -> ApiError {
    match error {
        orch_authz::http_origin::Error::Db(error) => ApiError::from(error),
        orch_authz::http_origin::Error::Access(error) => {
            ApiError::engine(orch_engine::EngineError::from(error))
        }
        _ => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "The original HTTP preparation identity has ended.",
        ),
    }
}

/// Retain the same request's raw/key minimum after native preparation awaits.
pub(crate) async fn current_preparation(
    ctx: &Ctx,
    conn: &mut Conn,
    source: &orch_authz::http_origin::HttpPreparation,
) -> ApiResult<()> {
    let original = ctx.authenticated_user()?;
    if source.user() != original.id()
        || source.operation() != ctx.route.name
        || source.credential() != &credential(ctx)?
        || source.as_admin() != original.as_admin
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "The native preparation changed its HTTP identity.",
        ));
    }
    let current = source
        .current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(preparation_error)?;
    ctx.publication.retain_ceiling(current.caller())?;
    ctx.publication.retain_deadline(current.until())?;
    current
        .check(ctx.app.clock.as_ref())
        .map_err(preparation_error)
}

/// A native producer may already own its exact captured original operation.
pub async fn capture_operation(
    ctx: &Ctx,
    conn: &mut Conn,
    origin: &CapturedOperation,
) -> ApiResult<HttpOrigin> {
    let original = ctx.authenticated_user()?;
    if origin.user() != original.id() {
        return Err(ApiError::internal(
            "The native source changed its HTTP actor.",
        ));
    }
    orch_engine::operation_scope::Prepared::new(ctx.route, origin.operation())
        .map_err(ApiError::internal)?;
    let identity =
        transaction_guard::current_http_identity(ctx, conn, HttpIdentityMode::Read).await?;
    transaction_guard::guard_user(&identity.user)?;
    let origin = origin
        .with_ceiling(&identity.caller)
        .map_err(ApiError::internal)?;
    let until = ctx
        .publication
        .retained_deadline()?
        .into_iter()
        .chain(identity.expires_at)
        .min();
    let captured = HttpOrigin::capture(
        origin,
        credential(ctx)?,
        original.as_admin,
        original.user.is_system_admin,
        until,
    )
    .map_err(mapped)?;
    let current = captured
        .current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(mapped)?;
    current.check(ctx.app.clock.as_ref()).map_err(mapped)?;
    ctx.publication.retain_ceiling(current.caller())?;
    Ok(captured)
}

/// Restore exact same-request identity at delivery without re-admitting a completed mutation.
pub async fn current(ctx: &Ctx, conn: &mut Conn, origin: &HttpOrigin) -> ApiResult<Current> {
    orch_engine::operation_scope::Prepared::new(ctx.route, origin.origin().operation())
        .map_err(ApiError::internal)?;
    let original = ctx.authenticated_user()?;
    if origin.origin().user() != original.id()
        || origin.credential() != &credential(ctx)?
        || origin.as_admin() != original.as_admin
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::AuthInvalidToken,
            "The native reply changed its original HTTP identity.",
        ));
    }
    let current = origin
        .current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(mapped)?;
    ctx.publication.retain_ceiling(current.caller())?;
    ctx.publication.retain_deadline(current.until())?;
    ctx.publication.retain_deadline(
        current
            .caller()
            .grant()
            .and_then(orch_authz::TokenGrant::expires_at),
    )?;
    current.check(ctx.app.clock.as_ref()).map_err(mapped)?;
    Ok(current)
}

/// Restore the stored command's accepted minimum for the same HTTP delivery.
/// This checks raw identity and retains response ceilings without replaying spent effects.
pub async fn delivery(
    ctx: &Ctx,
    conn: &mut Conn,
    authority: &CommandAuthority,
    id: orch_types::Id,
) -> ApiResult<(HttpOrigin, Current)> {
    let origin = authority
        .http_delivery(conn, ctx.app.clock.as_ref(), id)
        .await
        .map_err(|error| match error {
            orch_engine::EngineError::BeforeEffect(error) => {
                match error.downcast::<continuing::Error>() {
                    Ok(error) => mapped(*error),
                    Err(error) => ApiError::engine(orch_engine::EngineError::BeforeEffect(error)),
                }
            }
            error => ApiError::engine(error),
        })?;
    let current = current(ctx, conn, &origin).await?;
    Ok((origin, current))
}

/// Bind this identity conjunctively with the real native source's existing private command evidence.
pub fn bind(
    origin: &HttpOrigin,
    command: &Outgoing,
    native: CommandAuthority,
) -> ApiResult<CommandAuthority> {
    origin.command(command, native).map_err(ApiError::engine)
}

/// Clock-only tail for the same raw/key minimum, after any later native await.
pub fn check(ctx: &Ctx, current: &Current) -> ApiResult<()> {
    current.check(ctx.app.clock.as_ref()).map_err(mapped)
}

/// All new domain-native sources join the same HTTP envelope, without another channel arm.
pub fn bind_source(
    origin: &HttpOrigin,
    command: &Outgoing,
    source: orch_engine::operation_scope::http_command_source::CommandEvidence,
) -> ApiResult<CommandAuthority> {
    origin
        .command_source(command, source)
        .map_err(ApiError::engine)
}
