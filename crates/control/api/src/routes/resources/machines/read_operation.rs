//! Exact existing Machine-read targets; physical paths keep their current permission-page identity.

use crate::key_operations::targets;
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_authz::Caller;
use orch_db::Conn;

/// Only the three stored leaf readers register this producer. Machine stays the business
/// family while the current target is the hosted Host facade or the personal Machine.
/// Resolving identity grants no ordinary visibility; each handler keeps its native gate.
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let target = targets::machine_subject(conn, ctx.id("id")?).await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, KeyTargetKind::Object(ObjectKind::Machine), &[target])?;
        Ok(plan)
    })
}
