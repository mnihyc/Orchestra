//! Private in-memory provenance on existing native tickets, never a second ACL or transport token.

use crate::middleware::{
    authentication::Via,
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use orch_access_contract::key_scope::{KeyAction, KeyFacet};
use orch_authz::{Caller, CapturedCaller, CapturedOperation, Principal};
use orch_db::{Conn, queries::tokens};
use orch_types::{Clock, Id, Time};

/// Actual identity and raw session expiry only, never a reusable authorization proof.
pub(super) type CurrentCaller = (Caller, Option<Time>);

#[derive(Clone)]
enum Accepted {
    Operation(CapturedOperation),
    // Existing ordinary issuers not yet bound to operation metadata keep their real caller.
    // A modern key without actual accepted operation evidence cannot enter this branch.
    Caller(CapturedCaller),
}

/// The real accepted caller and exact browser session liveness, on the existing ticket only.
#[derive(Clone)]
pub(super) struct Origin {
    accepted: Accepted,
    session: Option<Id>,
}
impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TicketOrigin([private])")
    }
}

fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the ticket's original credential does not admit this operation",
    )
}

impl Origin {
    pub(super) fn capture(ctx: &Ctx) -> ApiResult<Self> {
        let user = ctx.user()?;
        let caller = match ctx.admitted_caller() {
            Some(caller) => caller.clone(),
            None => authorization::caller_of(user)?,
        };
        if !authorization::same_credential_origin(user, &caller) {
            return Err(refused());
        }
        if let Some(selection) = caller.admitted_operation() {
            let metadata =
                orch_api_contract::operations::for_route(&ctx.route).map_err(ApiError::internal)?;
            if !metadata
                .variants
                .iter()
                .any(|variant| variant.operation == selection)
            {
                return Err(refused());
            }
        }
        Self::from_caller(&caller, &user.via)
    }

    // Kept crate-private for existing pure transport fixtures; policy is identical to capture.
    pub(super) fn from_caller(caller: &Caller, via: &Via) -> ApiResult<Self> {
        let session = match via {
            Via::Session { session_id } if caller.grant().is_none() => Some(*session_id),
            Via::Token { token_id, .. }
                if caller
                    .grant()
                    .is_some_and(|grant| grant.token_ids().any(|id| id == *token_id)) =>
            {
                None
            }
            _ => return Err(refused()),
        };
        let accepted = if caller.admitted_operation().is_some() {
            Accepted::Operation(CapturedOperation::capture(caller).map_err(ApiError::internal)?)
        } else {
            if caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                return Err(refused());
            }
            Accepted::Caller(CapturedCaller::capture(caller))
        };
        Ok(Self { accepted, session })
    }

    pub(super) fn user(&self) -> Id {
        match &self.accepted {
            Accepted::Operation(origin) => origin.user(),
            Accepted::Caller(origin) => origin.user(),
        }
    }

    pub(super) fn operation(&self) -> Option<&str> {
        match &self.accepted {
            Accepted::Operation(origin) => Some(origin.operation()),
            Accepted::Caller(_) => None,
        }
    }

    /// Actual restored caller plus raw live-session deadline, not a new admission/lease.
    pub(super) async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> ApiResult<Option<(Caller, Option<Time>)>> {
        let session = if let Some(id) = self.session {
            let Some((admin, expires)) = tokens::lock_session_state(conn, id, self.user()).await?
            else {
                return Ok(None);
            };
            if expires <= clock.now() {
                return Ok(None);
            }
            Some((admin, Time::new(expires)))
        } else {
            None
        };
        let current = match &self.accepted {
            Accepted::Operation(origin) => origin.restore_current(conn, clock).await,
            Accepted::Caller(origin) => origin.restore_current(conn, clock).await,
        }
        .map_err(authorization::restoration_error)?;
        Ok(current.and_then(|caller| {
            let enabled = caller.as_admin() && session.is_some_and(|(admin, _)| admin);
            let caller = caller.with_admin_mode(enabled);
            let deadline = session.map(|(_, expires)| expires);
            deadline_allows(&caller, deadline, clock).then_some((caller, deadline))
        }))
    }

    /// The caller is current; this resolves the ACTUAL current Environment through the shared
    /// catalogue. Initial minting requires the complete plan; continuation can keep the original
    /// .control Read while its Manage is withdrawn, as the native protocol already permits.
    pub(super) async fn principal(
        &self,
        conn: &mut Conn,
        caller: Caller,
        environment: Id,
        selection: &str,
        clock: &dyn Clock,
    ) -> ApiResult<Option<(Principal, bool)>> {
        if let Some(operation) = self.operation() {
            if operation != selection {
                return Ok(None);
            }
            let plan = match crate::routes::interactive_tickets::core(conn, selection, environment)
                .await
            {
                Ok(plan) => plan,
                Err(error) if matches!(error.problem.status, 403 | 404 | 410) => return Ok(None),
                Err(error) => return Err(error),
            };
            let facts = plan.required_facets().map_err(ApiError::internal)?;
            let reads: Vec<KeyFacet> = facts
                .iter()
                .filter(|facet| facet.action == KeyAction::Read)
                .cloned()
                .collect();
            let manages: Vec<KeyFacet> = facts
                .iter()
                .filter(|facet| facet.action == KeyAction::Manage)
                .cloned()
                .collect();
            // These are exactly the original declared facets, not a nearby operation's proof.
            if reads.is_empty() || facts.len() != reads.len() + manages.len() {
                return Err(refused());
            }
            let manage = !manages.is_empty() && caller.permits_facets(&manages);
            let mut admitted = reads;
            if manage {
                admitted.extend(manages);
            }
            let caller = match caller.admit_facets(&admitted) {
                Ok(caller) => caller.into_caller(),
                Err(_) => return Ok(None),
            };
            let Some(principal) = current_principal(conn, caller, clock.time()).await? else {
                return Ok(None);
            };
            Ok(Some((principal, manage)))
        } else {
            // Compatibility issuers never invent an operation or a Session. They retain the
            // actual original caller; their unchanged ordinary door still decides permission.
            if caller
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
            {
                return Ok(None);
            }
            let Some(principal) = current_principal(conn, caller, clock.time()).await? else {
                return Ok(None);
            };
            Ok(Some((principal, true)))
        }
    }
}

/// Only an actual account/ordinary lifecycle loss is an unavailable principal. A database
/// conflict stays typed, and unexpected stored/database failures keep their normal error path.
async fn current_principal(
    conn: &mut Conn,
    caller: Caller,
    now: Time,
) -> ApiResult<Option<Principal>> {
    match Principal::load(conn, caller, now).await {
        Ok(principal) => Ok(Some(principal)),
        Err(
            orch_authz::Error::Suspended { .. }
            | orch_authz::Error::NoSuchUser(_)
            | orch_authz::Error::NotFound(_)
            | orch_authz::Error::Archived { .. },
        ) => Ok(None),
        Err(error) => Err(authorization::restoration_error(error)),
    }
}

/// A synchronous deadline veto only; policy/ordinary checks remain with the actual producer.
/// This is real HTTP-session expiry, never the short single-use ticket's redemption expiry.
pub(super) fn deadline_allows(caller: &Caller, session: Option<Time>, clock: &dyn Clock) -> bool {
    caller.key_deadline_allows(clock) && session.is_none_or(|end| clock.time().get() < end.get())
}
