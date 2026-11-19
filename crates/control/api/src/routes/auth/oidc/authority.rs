//! The accepted credential of an existing OIDC link, not a new authentication exchange.
//!
//! Provider proof and the one-use browser mark remain in the ordinary OIDC flow. The
//! stored policy contains no bearer secret, current target path, or admission proof.

use crate::key_operations::{admission::Prepared, targets};
use crate::middleware::authentication::Via;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::Guard;
use crate::registry::Ctx;
use orch_api_contract::auth::MeIdentityLink;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::routes::Endpoint;
use orch_authz::{Caller, CapturedOperation};
use orch_db::Conn;
use orch_db::queries::identity::SignInState;
use orch_types::{Clock, Id, Json};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkAuthority {
    origin: CapturedOperation,
    // Real browser-origin liveness is separate from the policy capture. A key is
    // retained as a key by its captured operation and never takes this session branch.
    session: Option<Id>,
}

fn browser_only() -> ApiError {
    ApiError::new(
        ErrorCode::AuthSessionRequired,
        "this legacy key cannot link or unlink a way to sign in",
    )
}

/// Compatibility only: explicit keys already passed the shared operation catalogue.
/// Never admit an unclassified new-rule caller merely because it is a key.
pub(super) fn check_legacy(caller: &Caller) -> ApiResult<()> {
    if caller
        .grant()
        .is_some_and(|grant| grant.requires_legacy_compatibility())
    {
        return Err(browser_only());
    }
    Ok(())
}

pub(super) fn check_caller(caller: &Caller) -> ApiResult<()> {
    check_legacy(caller)?;
    if caller.grant().is_some() {
        if caller
            .admitted_facets()
            .is_none_or(|facets| facets.is_empty())
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "the credential needs this operation's current scope admission",
            ));
        }
    }
    Ok(())
}

pub(super) fn admit_credential(ctx: &Ctx) -> ApiResult<()> {
    if ctx.user()?.is_session() {
        return Ok(());
    }
    let caller = ctx.admitted_caller().ok_or_else(browser_only)?;
    check_caller(caller)
}

pub(super) fn capture(guard: &Guard) -> ApiResult<Json> {
    let caller = guard.principal().caller();
    check_caller(caller)?;
    let session = match &guard.user().via {
        Via::Session { session_id } if caller.grant().is_none() => Some(*session_id),
        Via::Token { token_id, .. }
            if caller
                .grant()
                .is_some_and(|grant| grant.token_ids().any(|id| id == *token_id)) =>
        {
            None
        }
        _ => return Err(ApiError::internal("the link changed its credential origin")),
    };
    serde_json::to_value(LinkAuthority {
        origin: CapturedOperation::capture(caller).map_err(ApiError::internal)?,
        session,
    })
    .map_err(ApiError::internal)
}

/// Called after the existing personal writer locked this user. Key restore locks
/// every originating key/account through commit; the original browser session also
/// stays SHARE-locked. No external provider wait occurs under these locks.
pub(super) async fn restore(
    conn: &mut Conn,
    state: &SignInState,
    user: Id,
    clock: &dyn Clock,
) -> ApiResult<bool> {
    let Some(raw) = &state.link_authority else {
        return Ok(false);
    };
    let saved: LinkAuthority = serde_json::from_value(raw.clone()).map_err(ApiError::internal)?;
    if saved.origin.operation() != MeIdentityLink::ROUTE.name || saved.origin.user() != user {
        return Ok(false);
    }
    let Some(mut caller) = saved
        .origin
        .restore(conn, clock.time())
        .await
        .map_err(ApiError::internal)?
    else {
        return Ok(false);
    };
    match (saved.session, caller.grant().is_some()) {
        (Some(session), false) => {
            let live = orch_db::queries::tokens::lock_session_state(conn, session, user).await?;
            let Some((mode, _)) = live.filter(|(_, end)| *end > clock.now()) else {
                return Ok(false);
            };
            let as_admin = caller.as_admin() && mode;
            caller = caller.with_admin_mode(as_admin);
        }
        (None, true) => {
            // The policy restore can wait for a credential writer: check its actual
            // accepted/current deadline after that wait while its locks remain held.
            if caller
                .grant()
                .and_then(|grant| grant.expires_at())
                .is_some_and(|end| end.get() <= clock.now())
            {
                return Ok(false);
            }
        }
        _ => return Ok(false),
    }
    let target = targets::account(conn, user).await?;
    let mut plan = Prepared::new(MeIdentityLink::ROUTE, saved.origin.operation())?;
    plan.bind(0, target.object.kind, &[target])?;
    match plan.admit(&caller) {
        Ok(admitted) => {
            check_caller(&admitted.caller)?;
            Ok(true)
        }
        Err(error) if error.problem.status == 403 => Ok(false),
        Err(error) => Err(error),
    }
}
