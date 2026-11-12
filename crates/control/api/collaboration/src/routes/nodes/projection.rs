//! Credential bounds around the existing native Node producer, independent of its selector.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_authz::Caller;

use orch_db::Conn;

pub use orch_api_base::routes::nodes::projection::PRIMARY;

pub use orch_api_base::routes::nodes::projection::SOURCES;

pub use orch_api_base::routes::nodes::projection::Window;

pub use orch_api_base::routes::nodes::projection::Projection;

pub use orch_api_base::routes::nodes::projection::groups;

pub use orch_api_base::routes::nodes::projection::project_orgs;
pub(crate) fn bind(
    plan: &mut OperationPlan,
    slot: usize,
    rows: Vec<(KeyTargetKind, Vec<KeyResolvedTarget>)>,
) -> ApiResult<()> {
    if rows.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind_collection(slot, &rows)
    }
}
orch_api_base::__api_bind_7323c50a9b!();

/// Pure readers recheck their complete declared operation, including privileged
/// inspection and checked-empty proof, at the shared fresh response boundary.
/// Accepted mutation effects/receipts use the ordinary response helper instead.
pub(super) async fn disclose_read<T: Send + 'static>(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    response: OperationPlan,
    body: T,
    missing: &'static str,
) -> ApiResult<T> {
    let complete = response.shared().clone();
    response
        .read_response(ctx, conn, caller, move |_, reader| {
            Box::pin(async move {
                complete
                    .admit(reader.caller(), true)
                    .map_err(crate::key_operations::admission::binding_error)?;
                Ok(body)
            })
        })
        .await?
        .ok_or_else(|| ApiError::new(orch_api_contract::common::ErrorCode::AuthzDenied, missing))
}
