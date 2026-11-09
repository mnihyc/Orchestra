//! Re-admit the original saved review, independently of the caller confirming it.

use super::*;
use orch_api_contract::routes::Endpoint as _;
use orch_authz::CapturedOperation;
use orch_db::queries::jobs::forks::PreflightRow;

fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the review's original credential no longer admits its actual work",
    )
}

/// Only a synchronous deadline veto from the already current original-stage preparation.
/// It is not persisted or used to grant/replace any caller's scope.
#[derive(Clone)]
pub(in crate::routes::ports) struct CurrentReview {
    caller: Caller,
    session: Option<Time>,
}
impl CurrentReview {
    pub(in crate::routes::ports) fn check(&self, ctx: &Ctx) -> ApiResult<()> {
        if !self.caller.key_deadline_allows(ctx.app.clock.as_ref())
            || self
                .session
                .is_some_and(|end| ctx.app.clock.now() >= end.get())
        {
            return Err(refused());
        }
        Ok(())
    }
}

pub(in crate::routes::ports) async fn require(
    ctx: &Ctx,
    conn: &mut Conn,
    stored: &PreflightRow,
    request: &PortOpeningRequest,
) -> ApiResult<()> {
    if let Some(current) = current(ctx, conn, stored, request).await? {
        current.check(ctx)?;
    }
    Ok(())
}

/// Old ordinary reviews have no recorded credential boundary. New reviews always do;
/// a malformed or withdrawn recorded boundary never becomes legacy absence.
pub(in crate::routes::ports) async fn current(
    ctx: &Ctx,
    conn: &mut Conn,
    stored: &PreflightRow,
    request: &PortOpeningRequest,
) -> ApiResult<Option<CurrentReview>> {
    let Some(value) = stored.plan.get("creator_origin") else {
        return Ok(None);
    };
    let original: CapturedOperation =
        serde_json::from_value(value.clone()).map_err(ApiError::internal)?;
    if original.operation() != sources::OPENING_REVIEW.operation
        || original.user() != stored.created_by
    {
        return Err(refused());
    }
    let raw: Option<Id> = stored
        .plan
        .get("creator_session")
        .cloned()
        .map(serde_json::from_value::<Option<Id>>)
        .transpose()
        .map_err(ApiError::internal)?
        .flatten();
    let Some(mut caller) = original
        .restore_current(conn, ctx.app.clock.as_ref())
        .await
        .map_err(authorization::restoration_error)?
    else {
        return Err(refused());
    };
    let session = match (caller.grant().is_none(), raw) {
        (true, Some(id)) => {
            let (admin, end) =
                orch_db::queries::tokens::lock_session_state(conn, id, original.user())
                    .await?
                    .filter(|(_, end)| *end > ctx.app.clock.now())
                    .ok_or_else(refused)?;
            let enabled = caller.as_admin() && admin;
            caller = caller.with_admin_mode(enabled);
            Some(Time::new(end))
        }
        (false, None) => None,
        _ => return Err(refused()),
    };
    let environment = stored.environment_id.ok_or_else(refused)?;
    let mut facts = vec![primary(ctx, conn, environment).await?];
    let named = if request.kind == PortOpeningKind::Place {
        let place = request.place_id.ok_or_else(refused)?;
        let target = targets::node(conn, place).await?;
        vec![(target.object.kind, target)]
    } else {
        Vec::new()
    };
    facts.push(collection(Source::NamedPlace, named));
    let plan = native::bind_required_route(
        api::EnvironmentPortOpeningPreflight::ROUTE,
        &sources::OPENING_REVIEW,
        &facts,
    )?;
    // This is the saved stage's check, not the current HTTP operation's activity selection.
    let admitted = plan
        .shared()
        .admit(&caller, false)
        .map_err(crate::key_operations::admission::binding_error)?;
    let principal = authorization::load_principal(conn, admitted, ctx.app.clock.time()).await?;
    require_native(conn, &principal, environment, request).await?;
    let current = CurrentReview {
        caller: principal.caller().clone(),
        session,
    };
    current.check(ctx)?;
    Ok(Some(current))
}

async fn require_native(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    request: &PortOpeningRequest,
) -> ApiResult<()> {
    super::super::require(conn, principal, environment, Permission::PortsManage).await?;
    if request.kind == PortOpeningKind::Place
        && !core::place_view(conn, principal, request.place_id.ok_or_else(refused)?)
            .await
            .map_err(ApiError::internal)?
    {
        return Err(refused());
    }
    Ok(())
}
