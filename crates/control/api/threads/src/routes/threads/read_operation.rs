//! Typed current admission for the existing Thread stored-content producers.

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_authz::Caller;

use orch_db::Conn;
/// Stored-content readers share their declared selection and actual retained Thread identity.
/// Ordinary View/restoration authorization remains in the existing permission producer.
pub(super) fn prepare<'a, Q>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    _query: Q,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let selected = format!(
            "{}.{}",
            ctx.route.name,
            if ctx.inspect_deleted {
                "inspect_deleted"
            } else {
                "read"
            }
        );
        let mut plan = OperationPlan::new(ctx.route, &selected)?;
        let target = targets::node_identity(conn, ctx.id("id")?).await?;
        let kind = KeyTargetKind::Object(ObjectKind::Thread);
        if target.object.kind != kind {
            return Err(ApiError::not_found());
        }
        plan.bind(0, kind, &[target])?;
        Ok(plan)
    })
}

/// Re-resolve this same operation on both its read snapshot and current return checkpoint.
pub(super) async fn require_current(ctx: &Ctx, conn: &mut Conn) -> ApiResult<()> {
    let principal = ctx.operation_principal(conn).await?;
    authorization::require_on_path_in(ctx, conn, &principal, "thread.view").await
}

/// The existing return checkpoint, after releasing the content snapshot and its locks.
pub(super) async fn before_return(ctx: &Ctx) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    require_current(ctx, &mut conn).await
}
