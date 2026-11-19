//! One current background reader from an actual HTTP or WebSocket authenticator.
//! Native facts and scope are shared; this module never creates a Session authority.

use crate::key_operations::snapshot_read;
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::CurrentHttpIdentity;
use orch_access_contract::decisions::Decision;
use orch_api_contract::common::ErrorCode;
use orch_authz::{Caller, Principal};
use orch_db::Conn;
use orch_engine::background::{BackgroundError, Capture};
use orch_types::{Id, Time};
use std::collections::HashSet;

pub(crate) struct Frame {
    pub(crate) identity: CurrentHttpIdentity,
    pub(crate) principal: Principal,
    pub(crate) directories: HashSet<Id>,
    pub(crate) authority: i64,
    pub(crate) until: Option<Time>,
}

impl Frame {
    pub(crate) fn live(&self, app: &crate::state::App) -> bool {
        self.until.is_none_or(|until| app.clock.time() < until)
    }

    pub(crate) fn same_context(&self, other: &Self) -> bool {
        self.authority == other.authority
            && self.principal.caller() == other.principal.caller()
            && self.principal.second_step_hold() == other.principal.second_step_hold()
            && self.principal.suspension() == other.principal.suspension()
            && self.principal.groups() == other.principal.groups()
            && self.principal.member_of() == other.principal.member_of()
            && self.principal.has_system_admin_standing()
                == other.principal.has_system_admin_standing()
            && self.directories == other.directories
    }
}

pub(crate) async fn mapped(conn: &mut Conn, error: BackgroundError) -> ApiError {
    match error {
        BackgroundError::Denied(decision) => match *decision {
            Decision::Refused(refusal) => match refusals::denied(conn, refusal).await {
                Ok(error) | Err(error) => error,
            },
            Decision::Allowed { .. } => {
                ApiError::internal("an allowed background read was refused")
            }
        },
        BackgroundError::Internal(error) => snapshot_read::engine_error(error),
        BackgroundError::Scope => ApiError::new(
            ErrorCode::AuthzDenied,
            "the key does not allow this background read",
        ),
        BackgroundError::NotFound => ApiError::not_found(),
        BackgroundError::Stale => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the reviewed command list expired or changed; refresh it before End",
        )
        .with("refresh", "background"),
        BackgroundError::Invalid(message) => ApiError::invalid("", message),
    }
}

/// Preserve the actual reader's normal second-step/suspension refusal and recovery details.
pub(crate) async fn projection_error(
    conn: &mut Conn,
    principal: &Principal,
    error: BackgroundError,
) -> ApiError {
    match error {
        BackgroundError::Internal(orch_engine::EngineError::Authz(error)) => {
            authorization::engine_problem(conn, principal, error).await
        }
        other => mapped(conn, other).await,
    }
}

pub(crate) async fn prepare(
    app: &crate::state::App,
    conn: &mut Conn,
    mut identity: CurrentHttpIdentity,
    accepted: Option<&Caller>,
    thread: Id,
    capture: &Capture,
) -> ApiResult<Frame> {
    if let Some(accepted) = accepted {
        let same = identity.user.id() == accepted.user()
            && match (&identity.user.via, accepted.grant()) {
                (crate::middleware::authentication::Via::Session { .. }, None) => true,
                (crate::middleware::authentication::Via::Token { token_id, .. }, Some(grant)) => {
                    !accepted.as_admin() && grant.token_ids().any(|id| id == *token_id)
                }
                _ => false,
            };
        if !same {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the background reader changed credential origin",
            ));
        }
        let accepted = if accepted.grant().is_some() {
            orch_authz::CapturedCaller::capture(accepted)
                .restore_current(conn, app.clock.as_ref())
                .await
                .map_err(authorization::restoration_error)?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::AuthInvalidToken,
                        "the accepted key is no longer valid",
                    )
                })?
        } else {
            accepted.clone()
        };
        identity.caller = identity
            .caller
            .intersect_restrictions(&accepted)
            .map_err(ApiError::internal)?;
        identity.user.as_admin = identity.caller.as_admin();
    }
    let result = async {
        let (scope, directories) = orch_engine::background::read_scope_for_capture(
            conn,
            &identity.caller,
            thread,
            capture,
        )
        .await
        .map_err(|error| match error {
            BackgroundError::Scope => ApiError::new(
                ErrorCode::AuthzDenied,
                "the key does not allow this background read",
            ),
            BackgroundError::NotFound => ApiError::not_found(),
            BackgroundError::Internal(error) => snapshot_read::engine_error(error),
            other => ApiError::internal(other),
        })?;
        let caller = scope
            .admit(&identity.caller, true)
            .map_err(|error| match error {
                orch_engine::operation_scope::BindingError::Refused => ApiError::new(
                    ErrorCode::AuthzDenied,
                    "the key does not allow this background read",
                ),
                other => ApiError::internal(other),
            })?;
        let principal = authorization::load_principal(conn, caller, app.clock.time()).await?;
        let principal = crate::sign_in::second_step::hold(conn, &identity.user, principal).await?;
        let until = principal
            .read_deadline(conn)
            .await
            .map_err(|error| snapshot_read::engine_error(error.into()))?;
        let authority = orch_db::queries::access::authority_epoch(conn).await?;
        Ok::<_, ApiError>((principal, directories, until, authority))
    }
    .await;
    // Scope preparation, standing and assurance can all wait. Neither success nor errors
    // may borrow an expired raw credential or any expired accepted key ceiling.
    identity.check_deadline(app)?;
    let (principal, directories, until, authority) = result?;
    Ok(Frame {
        identity,
        principal,
        directories,
        authority,
        until,
    })
}
