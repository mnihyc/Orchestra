//! The genuine resource operation may compose its native work-grant projection.
//! Only the private in-process read extension selects this representation.

use super::*;

use crate::routes::resources::context;
pub(super) fn supports(route: &Route) -> bool {
    route.method == Method::GET
        && matches!(
            route.name,
            "ProviderGet" | "ProviderList" | "AgentGet" | "AgentList"
        )
}

pub(super) async fn serve(ctx: Ctx, request: context::Request) -> ApiResult<Response> {
    if !supports(&ctx.route)
        || ctx.ask_read.is_none()
        || ctx.user()?.as_admin
        || ctx.inspect_deleted
    {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "This is not a private contextual resource read",
        ));
    }
    let route = ctx.route;
    let audit_origin = crate::sign_in::origin(&ctx);
    let opening = ctx.opening;
    let session = ctx.user().ok().and_then(|user| match user.via {
        authentication::Via::Session { session_id } => Some(session_id),
        _ => None,
    });
    let scope = lock_scope(&ctx);
    let producing = ctx.clone();
    let work = async move {
        let mut tx = producing.app.db.begin_snapshot().await?;
        let identity = transaction_guard::current_http_identity(
            &producing,
            &mut tx,
            transaction_guard::HttpIdentityMode::Read,
        )
        .await?;
        transaction_guard::guard_user(&identity.user)?;
        let current = producing.for_operation_checkpoint(identity.user.clone(), identity.now.get());
        let prepared = context::prepare(&current, &mut tx, &identity.caller, request).await?;
        crate::ask::tools::check_plan(&current, &mut tx, prepared.plan()).await?;
        let shown = prepared.read(&current, &mut tx, &identity.caller).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        identity.check_deadline(&producing.app)?;
        transaction_guard::checked_snapshot_read();
        Ok::<_, ApiError>(shown)
    };
    let ((shown, guarded), _refused) = orch_db::pool::serving_watched(
        route.name,
        transaction_guard::watch(orch_db::audit::scope_origin(
            audit_origin,
            orch_db::queries::transaction_guard::request_scope(
                scope,
                orch_db::audit::scope_browser_opening(session, opening, work),
            ),
        )),
    )
    .await;
    let shown = shown?;
    transaction_guard::require_seen(&route, guarded)?;
    if ctx.key_receipt_required() {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "The native contextual read has not been admitted",
        ));
    }
    crate::middleware::inspection::finish(&ctx).await?;
    let mut response = axum::Json(shown).into_response();
    crate::middleware::inspection::headers(&mut response, &ctx)?;
    // Registry's outer publication::result checks the retained native projection
    // and exact original credential again immediately before these bytes return.
    Ok(response)
}
