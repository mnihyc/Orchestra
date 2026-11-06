//! The destination's actual stored node, shared by its existing guarded actions.

use super::*;
use crate::key_operations::{admission, targets};
use crate::registry::{OperationFuture, OperationPlan};
use orch_api_contract::common::ReceiptProjection;
use orch_api_contract::notifications::{DestinationUpdateReceipt, DestinationUpdateResponse};
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;

pub(super) struct PreparedDestination {
    response: OperationPlan,
    destination: Id,
    node: Id,
}

pub(crate) fn prepare<'a, Q, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    _body: B,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.shared()
            .compatible(caller)
            .map_err(admission::binding_error)?;
        let destination = ctx.id("id")?;
        // The initial lookup is statement-local; the real action guard retains this same
        // existing destination lock so the checked node cannot change before the writer.
        let node = rows::destination_node_for_write(conn, destination)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = targets::node(conn, node).await?;
        let facets = plan.variant().facets.clone();
        for (index, facet) in facets.iter().enumerate() {
            if !matches!(facet.target, TargetResolver::Resolved("destination_node")) {
                return Err(ApiError::internal(
                    "the destination action has an undeclared target role",
                ));
            }
            // Both the required action and optional response Read use their actual node kind.
            // The destination URL identity never becomes a node or another permission page.
            plan.bind(index, target.object.kind, std::slice::from_ref(&target))?;
        }
        let response = plan.clone();
        Ok(plan.with_data(PreparedDestination {
            response,
            destination,
            node,
        }))
    })
}

/// The same native destination view, only while the accepted/current caller has its Read.
/// A write without that disclosure remains a truthful committed receipt, not a fake Full.
pub(super) async fn updated(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedDestination,
    row: &DestinationRow,
) -> ApiResult<DestinationUpdateResponse> {
    if row.id != prepared.destination || row.node_id != prepared.node {
        return Err(ApiError::internal(
            "the guarded destination operand changed",
        ));
    }
    let row = row.clone();
    let shown = prepared
        .response
        .clone()
        .read_response(ctx, conn, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                views_of_with_principal(conn, reader, std::slice::from_ref(&row))
                    .await?
                    .into_iter()
                    .next()
                    .ok_or_else(ApiError::not_found)
            })
        })
        .await?;
    Ok(match (shown, ctx.key_receipt_required()) {
        (Some(destination), false) => DestinationUpdateResponse::Full(destination),
        _ => DestinationUpdateResponse::Receipt(DestinationUpdateReceipt {
            projection: ReceiptProjection::Receipt,
            destination_id: prepared.destination,
            node_id: prepared.node,
        }),
    })
}
