//! The destination's actual stored node, shared by its existing guarded actions.

use super::*;

use crate::key_operations::admission;

use crate::key_operations::targets;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::notifications::DestinationUpdateReceipt;

use orch_api_contract::notifications::DestinationUpdateResponse;

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
    render(ctx, conn, principal, prepared, row, false).await
}

async fn render(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedDestination,
    row: &DestinationRow,
    replaying: bool,
) -> ApiResult<DestinationUpdateResponse> {
    if row.id != prepared.destination || row.node_id != prepared.node {
        return Err(ApiError::internal(
            "the guarded destination operand changed",
        ));
    }
    let row = row.clone();
    let response = prepared.response.clone();
    let projection = if replaying {
        admission::ResponseProjection::Replay(
            response
                .variant()
                .facets
                .iter()
                .enumerate()
                .filter_map(|(index, facet)| {
                    (facet.usage == orch_api_contract::operations::FacetUsage::ResponseContent)
                        .then_some(index)
                })
                .collect(),
        )
    } else {
        admission::ResponseProjection::CompletedContent
    };
    let shown = response
        .read_response_projection(
            ctx,
            conn,
            principal.caller(),
            projection,
            move |conn, reader| {
                Box::pin(async move {
                    if replaying {
                        authorization::require(
                            conn,
                            reader,
                            row.node_id,
                            exact(Permission::SettingsManage),
                            Act::Use,
                            Policy::default(),
                        )
                        .await?;
                    }
                    views_of_with_principal(conn, reader, std::slice::from_ref(&row))
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(ApiError::not_found)
                })
            },
        )
        .await?;
    Ok(match (shown, ctx.key_receipt_required()) {
        (Some(destination), false) => DestinationUpdateResponse::full(destination),
        _ => DestinationUpdateResponse::Receipt(DestinationUpdateReceipt {
            projection: ReceiptProjection::Receipt,
            destination_id: prepared.destination,
            node_id: prepared.node,
        }),
    })
}

fn receipt(id: Id, node: Id) -> DestinationUpdateResponse {
    DestinationUpdateResponse::Receipt(DestinationUpdateReceipt {
        projection: ReceiptProjection::Receipt,
        destination_id: id,
        node_id: node,
    })
}

fn created_plan(
    ctx: &Ctx,
    node: Id,
    destination: Id,
    target: orch_access_contract::key_scope::KeyResolvedTarget,
) -> ApiResult<PreparedDestination> {
    let mut response = OperationPlan::new(ctx.route, ctx.route.name)?;
    for index in 0..response.variant().facets.len() {
        response.bind(index, target.object.kind, std::slice::from_ref(&target))?;
    }
    Ok(PreparedDestination {
        response,
        node,
        destination,
    })
}

pub(crate) fn creation() -> crate::middleware::idempotency::Creation<
    (),
    NodeDestinationCreateBody,
    DestinationUpdateResponse,
> {
    crate::middleware::idempotency::Creation {
        kind: "notification_destination",
        identity: |reply| {
            let (id, node) = match reply {
                DestinationUpdateResponse::Full(row) => (row.id, row.node.id),
                DestinationUpdateResponse::Receipt(row) => (row.destination_id, row.node_id),
            };
            Ok(orch_db::idempotency::creation::Created {
                kind: "notification_destination".into(),
                id,
                data: json!({"node":node}),
            })
        },
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                let node: Id =
                    serde_json::from_value(created.data.get("node").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted destination has no native node")
                    })?)
                    .map_err(ApiError::internal)?;
                let restricted = receipt(created.id, node);
                crate::key_operations::publication::receipt(&ctx, &restricted)?;
                let ctx = ctx.for_completed_replay_projection();
                let result = async {
                    let mut tx = ctx.app.db.begin_snapshot().await?;
                    let identity = transaction_guard::current_http_identity(
                        &ctx,
                        &mut tx,
                        transaction_guard::HttpIdentityMode::Read,
                    )
                    .await?;
                    let principal = authorization::load_principal(
                        &mut tx,
                        identity.caller.clone(),
                        identity.now,
                    )
                    .await?;
                    let principal =
                        crate::sign_in::second_step::hold(&mut tx, &identity.user, principal)
                            .await?;
                    let row = rows::destination(&mut tx, created.id)
                        .await?
                        .ok_or_else(ApiError::not_found)?;
                    if row.node_id != node || ctx.id("id")? != node {
                        return Err(ApiError::not_found());
                    }
                    let target = targets::node(&mut tx, node).await?;
                    let prepared = created_plan(&ctx, node, created.id, target)?;
                    let reply=render(&ctx,&mut tx,&principal,&prepared,&row,true).await?;
                    let named=matches!(&reply,DestinationUpdateResponse::Full(value) if value.node.name.is_some());
                    let earlier=prepared.response.clone();let accepted=principal.caller().clone();
                    tx.commit().await.map_err(DbError::from)?;identity.check_deadline(&ctx.app)?;
                    if matches!(&reply,DestinationUpdateResponse::Full(_)) {
                        crate::key_operations::publication::retain(&ctx,&accepted,(created.id,node,earlier,named),
                            |ctx,conn,current,(id,node,earlier,named)|Box::pin(async move {
                                let row=rows::destination(conn,*id).await?.filter(|row|row.node_id==*node).ok_or_else(ApiError::not_found)?;
                                let fresh=created_plan(ctx,*node,*id,targets::node(conn,*node).await?)?.response;
                                if !fresh.shared().covers_sources(earlier.shared()) {return Err(ApiError::not_found());}
                                let indices=fresh.variant().facets.iter().enumerate().filter_map(|(i,f)|
                                    (f.usage==orch_api_contract::operations::FacetUsage::ResponseContent).then_some(i)).collect();
                                let named=*named;
                                fresh.read_response_projection(ctx,conn,current.caller(),admission::ResponseProjection::Replay(indices),move|conn,p|Box::pin(async move {
                                    authorization::require(conn,p,row.node_id,exact(Permission::SettingsManage),Act::Use,Policy::default()).await?;
                                    if named {authorization::require(conn,p,row.node_id,authorization::VIEW,Act::Use,Policy::default()).await?;}
                                    Ok(())
                                })).await?.ok_or_else(ApiError::not_found)
                            }))?;
                    }
                    Ok::<_,ApiError>(reply)
                }
                .await;
                match result {
                    Ok(value) => Ok(value),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(restricted)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(super) async fn created(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    row: &DestinationRow,
) -> ApiResult<DestinationUpdateResponse> {
    let prepared = created_plan(
        ctx,
        row.node_id,
        row.id,
        targets::node(conn, row.node_id).await?,
    )?;
    render(ctx, conn, principal, &prepared, row, false).await
}
