//! Permanent-loss previews and typed confirmations under fresh resource authority.

use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::{Ctx, Registry};
use orch_api_contract::{common::ErrorCode, loss as api};
use orch_engine::loss::LossError;

mod recipients;
mod sources;

/// Binds reviewed declarations of permanent loss.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::LossPreflight, preflight)
        .on(api::LossDeclare, declare)
        .on_snapshot_body_read_with_optional_operation(
            api::LossRecoveryList,
            crate::key_operations::native::selected_body::<
                sources::Recovery,
                (),
                api::LossPreflightBody,
            >,
            crate::key_operations::native::selected_body_read::<
                sources::Recovery,
                (),
                api::LossPreflightBody,
            >,
        )
        .on_snapshot_read_with_optional_operation(
            api::OperationRecipientsGet,
            crate::key_operations::native::selected::<recipients::Recipients>,
            crate::key_operations::native::selected_read::<recipients::Recipients>,
        );
}

async fn require_recovery_target(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    target: api::LossTarget,
) -> ApiResult<()> {
    use orch_access_contract::permissions::{Permission, ResourceKind};
    match target {
        api::LossTarget::Host { id } => {
            authorization::require_resource(
                conn,
                principal,
                ResourceKind::Host,
                id,
                Permission::HostManage,
                orch_authz::Policy::default(),
            )
            .await?
        }
        api::LossTarget::Environment { id } => {
            authorization::require_resource(
                conn,
                principal,
                ResourceKind::Environment,
                id,
                Permission::EnvironmentManage,
                orch_authz::Policy::default(),
            )
            .await?
        }
        api::LossTarget::Machine { id } => {
            let owner = orch_db::queries::resources::machine(conn, id)
                .await?
                .and_then(|row| row.owner_id);
            if owner != Some(principal.user())
                && !orch_engine::operation_access::can_bypass(
                    conn,
                    principal,
                    orch_access_contract::decisions::ObjectRef {
                        kind: orch_access_contract::permissions::ObjectKind::Machine,
                        id,
                    },
                    Permission::MachineManage,
                )
                .await
                .map_err(ApiError::internal)?
            {
                return Err(ApiError::new(
                    ErrorCode::AuthzDenied,
                    "Only the machine's owner or a reviewed administrative act may recover its environments",
                ));
            }
            authorization::require_resource(
                conn,
                principal,
                ResourceKind::Machine,
                id,
                Permission::MachineManage,
                orch_authz::Policy::default(),
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) fn refused(error: LossError) -> ApiError {
    let code = match &error {
        LossError::NotFound => ErrorCode::NotFound,
        LossError::Denied => ErrorCode::AuthzDenied,
        LossError::Online => ErrorCode::PlanBlocked,
        LossError::Stale => ErrorCode::ConflictStaleRevision,
        LossError::Confirmation => ErrorCode::RequestInvalid,
        LossError::Db(_) | LossError::Internal(_) => return ApiError::internal(error),
    };
    ApiError::new(code, error.to_string())
}

async fn preflight(ctx: Ctx, (): (), body: api::LossPreflightBody) -> ApiResult<api::LossPlan> {
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let plan = ctx
        .app
        .engine
        .plan_loss(&mut tx, &principal, body.target)
        .await
        .map_err(refused)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(plan)
}

async fn declare(ctx: Ctx, (): (), body: api::LossDeclareBody) -> ApiResult<api::LossPlan> {
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let reviewed = orch_db::queries::jobs::forks::preflight(&mut tx, body.preflight_id)
        .await?
        .filter(|row| row.kind == "loss" && row.created_by == principal.user())
        .ok_or_else(ApiError::not_found)?;
    if reviewed.expires_at <= ctx.now {
        return Err(refused(LossError::Stale));
    }
    let target: api::LossTarget = serde_json::from_value(reviewed.plan["plan"]["target"].clone())
        .map_err(ApiError::internal)?;
    let machines = orch_db::queries::loss::inventory(&mut tx, target.kind(), target.id())
        .await?
        .ok_or_else(ApiError::not_found)?
        .machines;
    let plan = ctx
        .app
        .engine
        .declare_loss(&mut tx, &principal, &body)
        .await
        .map_err(refused)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    for machine in machines {
        ctx.app.link.let_go(machine).await;
    }
    ctx.app.engine.kick();
    Ok(plan)
}
