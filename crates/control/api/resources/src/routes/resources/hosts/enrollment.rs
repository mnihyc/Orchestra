//! Native host-token home facts; creation replies contain only the newly created credential.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::Facts;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::host_enrollment_sources as sources;

use orch_api_contract::host_enrollment_sources::Source;

use orch_authz::Caller;
pub(super) fn mapped(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match error {
        orch_engine::operation_scope::continuing::Error::Native(error) => {
            crate::key_operations::snapshot_read::engine_error(error)
        }
        _ => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original host enrollment source is no longer current",
        ),
    }
}

pub(super) fn fault(fault: crate::certificates::LinkFault) -> ApiError {
    ApiError::internal(format!("{}: {}", fault.code, fault.detail))
}

pub(super) fn validate(
    ctx: &Ctx,
    body: &api::HostEnrollmentTokenCreateBody,
) -> ApiResult<(orch_config::ServerConfig, Option<String>)> {
    let installation = super::super::machines::installation_config(ctx);
    super::super::machines::require_node_installable(&installation)?;
    let hint = body
        .name_hint
        .as_deref()
        .map(|hint| clean_name("name_hint", hint))
        .transpose()?;
    if body.home_id.is_none()
        && body.access == Some(orch_api_contract::access::NewResourceAccess::Home)
    {
        return Err(invalid(
            "access",
            "home access needs an org as the host's home",
        ));
    }
    Ok((installation, hint))
}

pub(super) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: api::HostEnrollmentTokenCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        validate(ctx, &body)?;
        let target = crate::routes::resources::creation_home::target(conn, body.home_id).await?;
        let plan: OperationPlan = native::bind_required(
            ctx,
            &sources::CREATE,
            &[Facts::one(
                Source::Home,
                KeyTargetKind::Object(ObjectKind::Host),
                target,
            )],
        )?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        crate::routes::resources::creation_home::require(
            conn,
            &principal,
            ResourceKind::Host,
            body.home_id,
            "a host is at home in an org",
            "only a system admin adds a host of the system",
        )
        .await?;
        Ok(plan)
    })
}
