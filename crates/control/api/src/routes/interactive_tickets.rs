//! Same native ticket bindings at initial admission and current delivery, from stored Environment.

use crate::{
    key_operations::{admission::Prepared, targets},
    middleware::requests::{ApiError, ApiResult},
    registry::Ctx,
};
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_db::Conn;
use orch_types::Id;

pub(crate) async fn core(
    conn: &mut Conn,
    selection: &str,
    environment: Id,
) -> ApiResult<orch_engine::operation_scope::Prepared> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    let mut plan = orch_engine::operation_scope::Prepared::for_selection(selection)
        .map_err(ApiError::internal)?;
    let count = plan.variant().facets.len();
    for index in 0..count {
        plan.bind(index, target.object.kind, std::slice::from_ref(&target))
            .map_err(ApiError::internal)?;
    }
    Ok(plan)
}

pub(super) async fn plan(
    ctx: &Ctx,
    conn: &mut Conn,
    selection: &str,
    environment: Id,
) -> ApiResult<Prepared> {
    let core = core(conn, selection, environment).await?;
    if !core.matches_route(&ctx.route) {
        return Err(ApiError::internal(
            "ticket binding belongs to another operation",
        ));
    }
    Ok(Prepared::from_shared(core).with_data(environment))
}
