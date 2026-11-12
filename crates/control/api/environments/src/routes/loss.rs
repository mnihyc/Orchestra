//! Permanent-loss previews and typed confirmations under fresh resource authority.

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::loss as api;

use orch_engine::loss::LossError;
/// Binds reviewed declarations of permanent loss.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            api::LossPreflight,
            crate::key_operations::native::effects::prepare::<preflight::Review>,
            preflight::run,
        )
        .on_with_operation(
            api::LossDeclare,
            crate::key_operations::native::effects::prepare::<declare::Confirm>,
            declare::run,
        )
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
orch_api_base::__api_bind_34f314958c!();

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

#[path = "loss/declare.rs"]
pub mod declare;

#[path = "loss/preflight.rs"]
pub mod preflight;

#[path = "loss/recipients.rs"]
pub mod recipients;

#[path = "loss/sources.rs"]
pub mod sources;
